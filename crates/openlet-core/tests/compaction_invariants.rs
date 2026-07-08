//! Property-based invariants on `runtime::compaction`.
//!
//! Locks: should_compact decision boundary (provider-actual override
//! and heuristic), `Run.keep` clamp behavior, `superseded_messages`
//! selection rule, and `build_compaction_projection` structural
//! invariants (system message kept, COMPACTION_REQUEST always present
//! and positioned before the kept tail).

use openlet_core::agent::{AgentDefinition, AgentSlug};
use openlet_core::projection::{LlmMessage, LlmRole};
use openlet_core::runtime::compaction::{
    COMPACTION_REQUEST, CompactDecision, PRESERVE_RECENT, build_compaction_projection,
    should_compact, superseded_messages,
};
use openlet_core::types::message::{Message, MessageId, Role};
use openlet_core::types::session::SessionId;
use proptest::prelude::*;

fn agent_with(context_window: u32, threshold: f32) -> AgentDefinition {
    AgentDefinition {
        slug: AgentSlug::new("general").unwrap(),
        title: "General".into(),
        description: String::new(),
        prompt_segments: None,
        tool_allowlist: Vec::new(),
        model_id: Some("test/model".into()),
        default_temperature: 0.0,
        context_window,
        compaction_threshold: threshold,
        compaction_summary_cap_tokens: 500,
        hidden: false,
    }
}

fn arb_role() -> impl Strategy<Value = LlmRole> {
    prop_oneof![
        Just(LlmRole::User),
        Just(LlmRole::Assistant),
        Just(LlmRole::Tool),
    ]
}

fn arb_msg() -> impl Strategy<Value = LlmMessage> {
    (
        arb_role(),
        prop::string::string_regex("[a-zA-Z0-9 ]{1,128}").unwrap(),
    )
        .prop_map(|(role, content)| {
            let tool_call_id = matches!(role, LlmRole::Tool).then(|| "tc-1".to_string());
            LlmMessage {
                role,
                content,
                reasoning: None,
                tool_calls: Vec::new(),
                tool_call_id,
            }
        })
}

fn system_msg(text: &str) -> LlmMessage {
    LlmMessage {
        role: LlmRole::System,
        content: text.to_string(),
        reasoning: None,
        tool_calls: Vec::new(),
        tool_call_id: None,
    }
}

fn mk_msg(role: Role) -> Message {
    Message {
        id: MessageId::new(),
        session_id: SessionId::new(),
        role,
        created_at: chrono::Utc::now(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]

    /// Provider-actual strictly below the threshold ALWAYS skips,
    /// regardless of message size. Provider value is authoritative.
    #[test]
    fn provider_actual_below_limit_always_skips(
        msgs in prop::collection::vec(arb_msg(), 1..16),
        ctx in 1024u32..32_768,
        actual_short in 0usize..100,
    ) {
        let agent = agent_with(ctx, 0.8);
        let limit = (f64::from(ctx) * 0.8) as usize;
        // Pick an actual strictly below the limit.
        let actual = actual_short.min(limit.saturating_sub(1));
        prop_assert_eq!(should_compact(&msgs, &agent, Some(actual), 0), CompactDecision::Skip);
    }

    /// Provider-actual at or above the threshold ALWAYS runs and the
    /// returned `keep` is bounded above by msgs.len() and by
    /// PRESERVE_RECENT.
    #[test]
    fn provider_actual_at_or_above_limit_runs_with_clamped_keep(
        msgs in prop::collection::vec(arb_msg(), 1..16),
        ctx in 1024u32..16_384,
        over in 0usize..16_384,
    ) {
        let agent = agent_with(ctx, 0.8);
        let limit = (f64::from(ctx) * 0.8) as usize;
        let actual = limit.saturating_add(over).max(limit);
        match should_compact(&msgs, &agent, Some(actual), 0) {
            CompactDecision::Run { keep } => {
                prop_assert!(keep <= msgs.len(), "keep {} exceeds msgs.len() {}", keep, msgs.len());
                prop_assert!(keep <= PRESERVE_RECENT, "keep {} exceeds PRESERVE_RECENT", keep);
                prop_assert_eq!(keep, msgs.len().min(PRESERVE_RECENT));
            }
            CompactDecision::Skip => prop_assert!(false, "expected Run at actual={}", actual),
        }
    }

    /// `build_compaction_projection` invariants:
    /// 1. exactly one `COMPACTION_REQUEST` user message in the output
    /// 2. system message (if any) is FIRST
    /// 3. last `keep` body messages all appear AFTER the request
    /// 4. output length never exceeds full.len() + 2 (system pass-through + request)
    #[test]
    fn build_compaction_projection_structure(
        body in prop::collection::vec(arb_msg(), 1..12),
        keep in 0usize..6,
        include_system in any::<bool>(),
    ) {
        let mut convo = Vec::new();
        if include_system {
            convo.push(system_msg("you are an assistant"));
        }
        convo.extend(body.iter().cloned());

        let proj = build_compaction_projection(&convo, keep);

        // Exactly one COMPACTION_REQUEST.
        let req_count = proj.iter().filter(|m| m.content == *COMPACTION_REQUEST).count();
        prop_assert_eq!(req_count, 1, "expected exactly one COMPACTION_REQUEST, got {}", req_count);

        // If a system message was present, it's at index 0.
        if include_system {
            prop_assert!(matches!(proj[0].role, LlmRole::System));
        }

        // Output length bounded by input length + 2 (sys + request).
        prop_assert!(
            proj.len() <= convo.len() + 2,
            "projection grew unexpectedly: {} > {} + 2",
            proj.len(),
            convo.len()
        );

        // The `keep` tail of the body must follow the COMPACTION_REQUEST,
        // but only when there are older messages to compact. When
        // body.len() <= keep, the source's defense-in-depth branch
        // emits the body first then the request (nothing to summarize
        // ahead of the kept tail).
        let req_idx = proj
            .iter()
            .position(|m| m.content == *COMPACTION_REQUEST)
            .expect("request present");
        if body.len() > keep {
            let tail_start = body.len() - keep;
            for tail_msg in &body[tail_start..] {
                let pos = proj
                    .iter()
                    .rposition(|m| m.content == tail_msg.content)
                    .expect("kept tail message present in projection");
                prop_assert!(
                    pos > req_idx,
                    "kept tail message must appear after the request (pos={}, req_idx={})",
                    pos,
                    req_idx,
                );
            }
        } else {
            // Defense-in-depth: request is appended last, after the body.
            prop_assert_eq!(
                req_idx,
                proj.len() - 1,
                "when body.len() <= keep, request must be the final message",
            );
        }
    }

    /// `superseded_messages` invariants:
    /// 1. result is a prefix of the non-system body (chronological)
    /// 2. result excludes the last `keep` non-system messages
    /// 3. result length = max(0, non_system_body.len() - keep)
    /// 4. system messages NEVER appear in the result
    #[test]
    fn superseded_messages_excludes_recent_and_system(
        n_user in 0usize..8,
        n_assistant in 0usize..8,
        n_tool in 0usize..4,
        with_system in any::<bool>(),
        keep in 0usize..8,
    ) {
        let mut msgs: Vec<Message> = Vec::new();
        if with_system {
            msgs.push(mk_msg(Role::System));
        }
        for _ in 0..n_user { msgs.push(mk_msg(Role::User)); }
        for _ in 0..n_assistant { msgs.push(mk_msg(Role::Assistant)); }
        for _ in 0..n_tool { msgs.push(mk_msg(Role::Tool)); }

        let body_len = n_user + n_assistant + n_tool;
        let body_ids: Vec<MessageId> = msgs
            .iter()
            .filter(|m| m.role != Role::System)
            .map(|m| m.id)
            .collect();

        let result = superseded_messages(&msgs, keep);

        // Length contract.
        let expected_len = body_len.saturating_sub(keep);
        prop_assert_eq!(result.len(), expected_len);

        // Content: first `expected_len` body ids in chronological order.
        prop_assert_eq!(&result, &body_ids[..expected_len]);

        // System message NEVER superseded.
        let system_id = msgs.iter().find(|m| m.role == Role::System).map(|m| m.id);
        if let Some(sid) = system_id {
            prop_assert!(!result.contains(&sid), "system message must not be superseded");
        }
    }

    /// Preservation invariant: the last `keep` body messages NEVER
    /// appear in the superseded list. Pivots on the same construction
    /// as the prior property but asserts the complement directly.
    #[test]
    fn preserved_tail_is_never_superseded(
        n_body in 1usize..16,
        keep in 0usize..16,
    ) {
        let msgs: Vec<Message> = (0..n_body).map(|_| mk_msg(Role::User)).collect();
        let result = superseded_messages(&msgs, keep);
        let body_ids: Vec<MessageId> = msgs.iter().map(|m| m.id).collect();

        let effective_keep = keep.min(n_body);
        let tail_start = n_body - effective_keep;
        for kept_id in &body_ids[tail_start..] {
            prop_assert!(
                !result.contains(kept_id),
                "preserved tail msg id {:?} appeared in superseded list",
                kept_id,
            );
        }
    }
}
