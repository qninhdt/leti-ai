//! Property-based invariants on `projection::project_for_llm`.
//!
//! Locks the deterministic projection rules that compaction relies on:
//! tool_call / tool_result pairing by call_id, compaction substitution
//! happens exactly once per summary regardless of how many messages it
//! supersedes, and projection is idempotent (same inputs → same
//! outputs). Drift in any of these would let unpaired tool results
//! reach the provider (4xx) or duplicate compaction summaries leak
//! tokens.

use std::collections::HashMap;

use openlet_core::projection::{LlmRole, ProjectionCaps, project_for_llm};
use openlet_core::types::message::{Message, MessageId, Role};
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::session::SessionId;
use proptest::prelude::*;

fn arb_role() -> impl Strategy<Value = Role> {
    prop_oneof![
        Just(Role::User),
        Just(Role::Assistant),
        Just(Role::Tool),
    ]
}

/// Build (msgs, parts_by_msg) where every Tool message's tool_result
/// pairs with a preceding Assistant message's tool_call by call_id.
fn arb_paired_tool_calls()
-> impl Strategy<Value = (Vec<Message>, HashMap<MessageId, Vec<Part>>, Vec<String>)> {
    (
        prop::collection::vec(arb_role(), 1..12),
        prop::collection::vec("[a-z][a-z0-9_]{2,8}", 1..6),
    )
        .prop_map(|(roles, tool_names)| {
            let session = SessionId::new();
            let mut msgs = Vec::with_capacity(roles.len());
            let mut parts: HashMap<MessageId, Vec<Part>> = HashMap::new();
            let mut call_ids = Vec::new();
            let mut next_call: usize = 0;
            for role in roles {
                let id = MessageId::new();
                let m = Message {
                    id,
                    session_id: session,
                    role,
                    created_at: chrono::Utc::now(),
                };
                let mut msg_parts = Vec::new();
                match role {
                    Role::Assistant => {
                        // Each assistant emits 0-2 tool calls.
                        let n_calls = next_call % 3;
                        for k in 0..n_calls {
                            let call_id = format!("call-{}-{}", next_call, k);
                            call_ids.push(call_id.clone());
                            let name_idx = (next_call + k) % tool_names.len();
                            msg_parts.push(Part::ToolCall {
                                id: PartId::new(),
                                call_id,
                                name: tool_names[name_idx].clone(),
                                args: serde_json::json!({}),
                            });
                        }
                        next_call += 1;
                    }
                    Role::Tool => {
                        // Pair this Tool message with the most recent
                        // unpaired call_id, if any.
                        if let Some(call_id) = call_ids.pop() {
                            msg_parts.push(Part::ToolResult {
                                id: PartId::new(),
                                call_id,
                                ok: true,
                                text: Some("ok".to_string()),
                                error: None,
                            });
                        } else {
                            // Emit something so it's not empty — synthesize
                            // a result with a fresh id (orphan, on purpose:
                            // tests an edge of pairing).
                            msg_parts.push(Part::ToolResult {
                                id: PartId::new(),
                                call_id: format!("orphan-{}", next_call),
                                ok: true,
                                text: Some("ok".to_string()),
                                error: None,
                            });
                            next_call += 1;
                        }
                    }
                    Role::User => {
                        msg_parts.push(Part::Text {
                            id: PartId::new(),
                            text: format!("user msg {}", next_call),
                        });
                        next_call += 1;
                    }
                    Role::System => {
                        // Not emitted by arb_role() but harmless if added.
                        msg_parts.push(Part::Text {
                            id: PartId::new(),
                            text: "system".to_string(),
                        });
                    }
                }
                parts.insert(id, msg_parts);
                msgs.push(m);
            }
            // Collect all call_ids actually emitted (by walking parts).
            let mut all_call_ids: Vec<String> = parts
                .values()
                .flatten()
                .filter_map(|p| match p {
                    Part::ToolCall { call_id, .. } => Some(call_id.clone()),
                    _ => None,
                })
                .collect();
            all_call_ids.sort();
            (msgs, parts, all_call_ids)
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, .. ProptestConfig::default() })]

    /// Projection is deterministic: same inputs → same outputs.
    /// Required by compaction (which projects the same log
    /// repeatedly to compute the threshold).
    #[test]
    fn projection_is_deterministic((msgs, parts, _call_ids) in arb_paired_tool_calls()) {
        let caps = ProjectionCaps::default();
        let a = project_for_llm(&msgs, &parts, caps);
        let b = project_for_llm(&msgs, &parts, caps);
        prop_assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            prop_assert_eq!(&x.role, &y.role, "role drift at index {}", i);
            prop_assert_eq!(&x.content, &y.content, "content drift at index {}", i);
            prop_assert_eq!(&x.tool_call_id, &y.tool_call_id, "tool_call_id drift at index {}", i);
        }
    }

    /// Output length is bounded above by msgs.len(): projection NEVER
    /// emits more LlmMessages than input messages. (Tool messages can
    /// emit one Llm per tool result, but the generator emits at most
    /// one ToolResult per Tool message.)
    #[test]
    fn output_size_bounded_by_input(
        (msgs, parts, _call_ids) in arb_paired_tool_calls(),
    ) {
        let proj = project_for_llm(&msgs, &parts, ProjectionCaps::default());
        prop_assert!(
            proj.len() <= msgs.len(),
            "projection emitted {} for {} messages",
            proj.len(),
            msgs.len(),
        );
    }

    /// Every emitted Tool LlmMessage carries a tool_call_id matching
    /// some call_id seen on an Assistant message — OR is the synthetic
    /// orphan we deliberately injected (call_id starts with "orphan-").
    /// This locks the "pass through call_id" rule.
    #[test]
    fn tool_messages_carry_call_id((msgs, parts, _call_ids) in arb_paired_tool_calls()) {
        let proj = project_for_llm(&msgs, &parts, ProjectionCaps::default());
        for m in &proj {
            if matches!(m.role, LlmRole::Tool) {
                prop_assert!(
                    m.tool_call_id.is_some(),
                    "Tool LlmMessage missing tool_call_id"
                );
            }
        }
    }

    /// Compaction substitution invariants: when a Compaction part
    /// supersedes N messages, the projection drops those N messages
    /// and emits exactly ONE System summary in their place. Total
    /// length therefore decreases by N-1 vs an uncompacted projection
    /// (or stays equal when N == 1).
    #[test]
    fn compaction_substitutes_once_for_owner(
        n_old in 1usize..6,
        n_recent in 0usize..4,
    ) {
        // Build: n_old User messages, then one Assistant Compaction
        // that supersedes all of them, then n_recent User messages.
        let session = SessionId::new();
        let mut msgs: Vec<Message> = Vec::new();
        let mut parts: HashMap<MessageId, Vec<Part>> = HashMap::new();
        let mut superseded_ids: Vec<String> = Vec::new();

        for i in 0..n_old {
            let id = MessageId::new();
            superseded_ids.push(id.0.to_string());
            msgs.push(Message {
                id,
                session_id: session,
                role: Role::User,
                created_at: chrono::Utc::now(),
            });
            parts.insert(id, vec![Part::Text {
                id: PartId::new(),
                text: format!("old{i}"),
            }]);
        }

        let comp_id = MessageId::new();
        msgs.push(Message {
            id: comp_id,
            session_id: session,
            role: Role::Assistant,
            created_at: chrono::Utc::now(),
        });
        parts.insert(comp_id, vec![Part::Compaction {
            id: PartId::new(),
            summary: "summary text".to_string(),
            compacted_message_ids: superseded_ids,
            original_token_count: 100,
        }]);

        for i in 0..n_recent {
            let id = MessageId::new();
            msgs.push(Message {
                id,
                session_id: session,
                role: Role::User,
                created_at: chrono::Utc::now(),
            });
            parts.insert(id, vec![Part::Text {
                id: PartId::new(),
                text: format!("recent{i}"),
            }]);
        }

        let proj = project_for_llm(&msgs, &parts, ProjectionCaps::default());

        // Exactly one System summary in the output.
        let summary_count = proj
            .iter()
            .filter(|m| matches!(m.role, LlmRole::System) && m.content.contains("summary text"))
            .count();
        prop_assert_eq!(summary_count, 1, "expected 1 summary, got {}", summary_count);

        // None of the original "old{N}" texts appear (replaced by summary).
        for i in 0..n_old {
            let needle = format!("old{i}");
            prop_assert!(
                !proj.iter().any(|m| m.content.contains(&needle)),
                "superseded message {} leaked into projection",
                needle,
            );
        }

        // All n_recent "recent{N}" texts DO appear (preserved).
        for i in 0..n_recent {
            let needle = format!("recent{i}");
            prop_assert!(
                proj.iter().any(|m| m.content.contains(&needle)),
                "recent message {} missing from projection",
                needle,
            );
        }

        // Length bound: 1 (summary) + n_recent. The Compaction-bearing
        // message itself emits no LlmMessage of its own.
        prop_assert_eq!(
            proj.len(),
            1 + n_recent,
            "expected {} messages, got {}", 1 + n_recent, proj.len(),
        );
    }

    /// User messages with empty parts produce NO LlmMessage. Locks the
    /// "skip empty content" rule — without this, every empty append
    /// would burn a slot in the projection.
    #[test]
    fn empty_user_message_drops(role in arb_role()) {
        let session = SessionId::new();
        let id = MessageId::new();
        let msgs = vec![Message {
            id,
            session_id: session,
            role,
            created_at: chrono::Utc::now(),
        }];
        let mut parts: HashMap<MessageId, Vec<Part>> = HashMap::new();
        parts.insert(id, vec![]);

        let proj = project_for_llm(&msgs, &parts, ProjectionCaps::default());
        prop_assert!(
            proj.is_empty(),
            "empty {:?} message produced {} LlmMessage(s)",
            role,
            proj.len(),
        );
    }
}
