//! Per-session model override reaches the provider, and provider
//! capabilities are computed for the resolved model (not a hardcoded
//! default) — so a vision-pinned session doesn't degrade attachments.

mod common;

use std::sync::Arc;

use openlet_core::adapters::{EventSink, MemoryStore, ModelProvider};
use openlet_core::projection::{LlmMessage, LlmRole};
use openlet_core::runtime::{ConversationRuntime, RuntimeConfig, TurnInput};
use openlet_core::types::session::SessionId;
use tokio_util::sync::CancellationToken;

use common::mock_event_sink::RecordingEventSink;
use common::mock_memory::MockMemoryStore;
use common::mock_provider::ScriptedProvider;

fn user_msg(text: &str) -> Vec<LlmMessage> {
    vec![LlmMessage {
        role: LlmRole::User,
        content: text.into(),
        reasoning: None,
        tool_calls: vec![],
        tool_call_id: None,
    }]
}

/// A `TurnInput.model = Some(X)` must send X to the provider, NOT the
/// runtime's configured default. Proves the per-session override plumbs
/// through `run_turn` end to end.
#[tokio::test]
async fn session_model_override_reaches_provider() {
    let provider = Arc::new(ScriptedProvider::new());
    provider.push_text_turn("ok");
    let memory = Arc::new(MockMemoryStore::new());
    let events = Arc::new(RecordingEventSink::new());

    let runtime = ConversationRuntime::new(
        provider.clone() as Arc<dyn ModelProvider>,
        memory as Arc<dyn MemoryStore>,
        events as Arc<dyn EventSink>,
        RuntimeConfig::new("default/model".into()),
    );

    let input = TurnInput {
        session_id: SessionId::new(),
        messages: user_msg("hi"),
        system_prompt: None,
        model: Some("anthropic/claude-sonnet-4-6".into()),
        max_tokens: None,
        temperature: None,
        tools: vec![],
    };
    runtime
        .run_turn(input, CancellationToken::new())
        .await
        .expect("turn ok");

    assert_eq!(
        provider.seen_models(),
        vec!["anthropic/claude-sonnet-4-6".to_string()],
        "the per-session model override must reach the provider verbatim"
    );
}

/// With no override, the provider receives the runtime's default model.
#[tokio::test]
async fn absent_override_falls_back_to_default_model() {
    let provider = Arc::new(ScriptedProvider::new());
    provider.push_text_turn("ok");
    let memory = Arc::new(MockMemoryStore::new());
    let events = Arc::new(RecordingEventSink::new());

    let runtime = ConversationRuntime::new(
        provider.clone() as Arc<dyn ModelProvider>,
        memory as Arc<dyn MemoryStore>,
        events as Arc<dyn EventSink>,
        RuntimeConfig::new("default/model".into()),
    );

    let input = TurnInput {
        session_id: SessionId::new(),
        messages: user_msg("hi"),
        system_prompt: None,
        model: None,
        max_tokens: None,
        temperature: None,
        tools: vec![],
    };
    runtime
        .run_turn(input, CancellationToken::new())
        .await
        .expect("turn ok");

    assert_eq!(provider.seen_models(), vec!["default/model".to_string()]);
}

/// Capabilities are model-keyed: the provider reports vision support for
/// a vision model and not for a text-only one. This is what lets the
/// server compute `ProjectionCaps` for the session's resolved model
/// rather than a hardcoded default.
#[test]
fn capabilities_track_the_resolved_model() {
    let provider = ScriptedProvider::new().with_vision_marker("vision");
    assert!(
        provider.capabilities("vendor/vision-pro").supports_vision,
        "a vision model must report vision support"
    );
    assert!(
        !provider.capabilities("vendor/text-only").supports_vision,
        "a non-vision model must NOT report vision support"
    );
}
