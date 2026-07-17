//! Phase-07 compaction trigger tests.

use leti_core::agent::{AgentDefinition, AgentSlug, PromptSegments};
use leti_core::projection::{LlmMessage, LlmRole};
use leti_core::runtime::compaction::{CompactDecision, PRESERVE_RECENT, should_compact};

fn agent_with_window(window: u32, threshold: f32) -> AgentDefinition {
    AgentDefinition {
        slug: AgentSlug::new("general").unwrap(),
        title: "General".into(),
        description: String::new(),
        prompt_segments: Some(PromptSegments::default()),
        tool_allowlist: Vec::new(),
        model_id: Some("test".into()),
        default_temperature: 0.0,
        context_window: window,
        compaction_threshold: threshold,
        compaction_summary_cap_tokens: 500,
        hidden: false,
    }
}

fn user_msg(body: &str) -> LlmMessage {
    LlmMessage {
        role: LlmRole::User,
        content: body.to_string(),
        reasoning: None,
        tool_calls: Vec::new(),
        tool_call_id: None,
    }
}

#[test]
fn skips_under_threshold() {
    let agent = agent_with_window(10_000, 0.8);
    let convo = vec![user_msg("hello")];
    assert_eq!(
        should_compact(&convo, &agent, None, 0),
        CompactDecision::Skip
    );
}

#[test]
fn fires_via_provider_actual() {
    let agent = agent_with_window(10_000, 0.8);
    let convo = vec![user_msg("hi")];
    let d = should_compact(&convo, &agent, Some(8_500), 0);
    match d {
        CompactDecision::Run { keep } => assert!(keep <= PRESERVE_RECENT),
        _ => panic!("expected Run"),
    }
}

#[test]
fn fires_via_heuristic() {
    let agent = agent_with_window(1_000, 0.8);
    let big = "x".repeat(4_000); // 4000 / 4 = 1000 tokens, threshold 800
    let convo = vec![user_msg(&big)];
    assert!(matches!(
        should_compact(&convo, &agent, None, 0),
        CompactDecision::Run { .. }
    ));
}

#[test]
fn keep_capped_by_history_length() {
    let agent = agent_with_window(1_000, 0.8);
    let big = "x".repeat(4_000);
    let convo = vec![user_msg(&big), user_msg(&big)]; // only 2 messages
    match should_compact(&convo, &agent, Some(2_000), 0) {
        CompactDecision::Run { keep } => assert_eq!(keep, 2),
        _ => panic!("expected Run"),
    }
}
