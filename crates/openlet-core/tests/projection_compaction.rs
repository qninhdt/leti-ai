//! Phase-07 projection-substitution test: a Compaction part replaces the
//! listed `compacted_message_ids` in the projected LLM messages.

use std::collections::HashMap;

use chrono::Utc;
use openlet_core::projection::{LlmRole, ProjectionCaps, project_for_llm};
use openlet_core::types::message::{Message, MessageId, Role};
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::session::SessionId;

fn msg(role: Role, sid: SessionId) -> Message {
    Message {
        id: MessageId::new(),
        session_id: sid,
        role,
        created_at: Utc::now(),
    }
}

fn text_part(text: &str) -> Part {
    Part::Text {
        id: PartId::new(),
        text: text.to_owned(),
    }
}

#[test]
fn compaction_substitutes_old_messages() {
    let sid = SessionId::new();
    let m0 = msg(Role::User, sid);
    let m1 = msg(Role::Assistant, sid);
    let m2 = msg(Role::User, sid);
    let comp_owner = msg(Role::Assistant, sid);
    let m3 = msg(Role::User, sid); // recent — kept

    let mut parts = HashMap::new();
    parts.insert(m0.id, vec![text_part("first user")]);
    parts.insert(m1.id, vec![text_part("first assistant")]);
    parts.insert(m2.id, vec![text_part("second user")]);
    parts.insert(
        comp_owner.id,
        vec![Part::Compaction {
            id: PartId::new(),
            summary: "summary of older turns".to_owned(),
            compacted_message_ids: vec![
                m0.id.0.to_string(),
                m1.id.0.to_string(),
                m2.id.0.to_string(),
            ],
            original_token_count: 1234,
        }],
    );
    parts.insert(m3.id, vec![text_part("recent user")]);

    let msgs = vec![m0, m1, m2, comp_owner, m3];
    let projected = project_for_llm(&msgs, &parts, ProjectionCaps::default());

    // Expect: 1 system summary + 1 user (recent) = 2 messages.
    assert_eq!(projected.len(), 2, "got {projected:?}");
    assert!(matches!(projected[0].role, LlmRole::System));
    assert!(projected[0].content.contains("summary of older turns"));
    assert!(matches!(projected[1].role, LlmRole::User));
    assert_eq!(projected[1].content, "recent user");
}

#[test]
fn projection_unchanged_without_compaction() {
    let sid = SessionId::new();
    let m0 = msg(Role::User, sid);
    let m1 = msg(Role::Assistant, sid);
    let mut parts = HashMap::new();
    parts.insert(m0.id, vec![text_part("hi")]);
    parts.insert(m1.id, vec![text_part("hello")]);
    let projected = project_for_llm(&[m0, m1], &parts, ProjectionCaps::default());
    assert_eq!(projected.len(), 2);
}

/// Regression for F-1: when the synthetic compaction-request user message
/// AND the verbatim summary text from the compaction turn are both listed
/// in `compacted_message_ids`, neither leaks into the next projection.
#[test]
fn synthetic_request_and_verbatim_summary_substituted() {
    use openlet_core::runtime::compaction::COMPACTION_REQUEST;
    let sid = SessionId::new();
    let original = msg(Role::User, sid);
    let synth = msg(Role::User, sid);
    let verbatim = msg(Role::Assistant, sid); // raw summary text
    let comp_owner = msg(Role::Assistant, sid); // Part::Compaction holder
    let recent = msg(Role::User, sid);

    let mut parts = HashMap::new();
    parts.insert(original.id, vec![text_part("the original turn")]);
    parts.insert(synth.id, vec![text_part(COMPACTION_REQUEST)]);
    parts.insert(verbatim.id, vec![text_part("- Goal: x\n- Files: y")]);
    parts.insert(
        comp_owner.id,
        vec![Part::Compaction {
            id: PartId::new(),
            summary: "- Goal: x\n- Files: y".to_owned(),
            compacted_message_ids: vec![
                original.id.0.to_string(),
                synth.id.0.to_string(),
                verbatim.id.0.to_string(),
            ],
            original_token_count: 1234,
        }],
    );
    parts.insert(recent.id, vec![text_part("recent prompt")]);

    let msgs = vec![original, synth, verbatim, comp_owner, recent];
    let projected = project_for_llm(&msgs, &parts, ProjectionCaps::default());

    // Expect: 1 system summary + 1 user (recent) = 2.
    assert_eq!(projected.len(), 2, "got {projected:?}");
    let any_synth_leak = projected
        .iter()
        .any(|m| m.content.contains("Summarize the conversation history above"));
    assert!(!any_synth_leak, "synthetic request leaked: {projected:?}");
    // The summary appears exactly once (in the substituted system message).
    let summary_hits: usize = projected
        .iter()
        .filter(|m| m.content.contains("- Goal: x"))
        .count();
    assert_eq!(summary_hits, 1, "summary appeared {summary_hits} times");
}
