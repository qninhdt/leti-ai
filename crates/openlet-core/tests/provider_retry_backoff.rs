//! Provider retry/backoff at the runtime↔provider boundary.
//!
//! The runtime wraps the chat-stream OPEN call in a bounded retry that
//! consumes transient `ProviderError`s (rate-limit, network/5xx),
//! honoring `Retry-After` when present and capping attempts + total
//! sleep. Non-retryable errors (auth, decode) bubble immediately.
//!
//! These tests use a fast retry config (1ms base, short deadline) so
//! the backoff path runs without slowing the suite.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::mock_event_sink::RecordingEventSink;
use common::mock_memory::MockMemoryStore;
use common::mock_provider::ScriptedProvider;

use openlet_core::adapters::{EventSink, MemoryStore, ModelProvider};
use openlet_core::error::ProviderError;
use openlet_core::runtime::{ConversationRuntime, RetryConfig, RuntimeConfig, TurnInput};
use openlet_core::types::session::SessionId;
use tokio_util::sync::CancellationToken;

/// Boot a runtime with a fast, deterministic retry policy so backoff
/// sleeps don't slow the suite. The provider is shared so the test can
/// script open-errors + a success turn before driving.
fn boot(provider: Arc<ScriptedProvider>) -> ConversationRuntime {
    let memory: Arc<dyn MemoryStore> = Arc::new(MockMemoryStore::new());
    let events: Arc<dyn EventSink> = Arc::new(RecordingEventSink::new());
    let mut cfg = RuntimeConfig::new("test-model".to_string());
    cfg.retry = RetryConfig {
        max_attempts: 4,
        base_delay: Duration::from_millis(1),
        total_deadline: Duration::from_secs(5),
    };
    let provider_dyn: Arc<dyn ModelProvider> = provider;
    ConversationRuntime::new(provider_dyn, memory, events, cfg)
}

fn turn(session_id: SessionId) -> TurnInput {
    TurnInput {
        session_id,
        messages: vec![openlet_core::projection::LlmMessage {
            role: openlet_core::projection::LlmRole::User,
            content: "hi".into(),
            reasoning: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system_prompt: None,
        model: None,
        max_tokens: None,
        temperature: None,
        tools: vec![],
    }
}

#[tokio::test]
async fn rate_limit_then_success_retries_and_completes() {
    let provider = Arc::new(ScriptedProvider::new());
    // First open fails with a 429 carrying Retry-After; second succeeds.
    provider.push_open_error(ProviderError::RateLimit { retry_after_ms: 5 });
    provider.push_text_turn("recovered");

    let runtime = boot(provider.clone());
    let outcome = runtime
        .run_turn(turn(SessionId::new()), CancellationToken::new())
        .await
        .expect("turn should succeed after one retry");

    // EndTurn = the scripted success turn ran.
    assert_eq!(
        outcome.finish_reason,
        openlet_core::adapters::model_provider::FinishReason::EndTurn
    );
    // Two opens: the failed 429 + the successful retry.
    assert_eq!(provider.call_count(), 2, "expected exactly one retry");
}

#[tokio::test]
async fn transient_5xx_retried_until_success() {
    let provider = Arc::new(ScriptedProvider::new());
    // Two transient network/5xx failures, then success.
    provider.push_open_error(ProviderError::Network("502 bad gateway".into()));
    provider.push_open_error(ProviderError::Network("503 unavailable".into()));
    provider.push_text_turn("ok");

    let runtime = boot(provider.clone());
    let outcome = runtime
        .run_turn(turn(SessionId::new()), CancellationToken::new())
        .await
        .expect("turn should succeed after two retries");

    assert_eq!(
        outcome.finish_reason,
        openlet_core::adapters::model_provider::FinishReason::EndTurn
    );
    assert_eq!(provider.call_count(), 3, "expected exactly two retries");
}

#[tokio::test]
async fn auth_error_is_not_retried() {
    let provider = Arc::new(ScriptedProvider::new());
    // Auth failure must bubble on the first attempt — retrying a bad key
    // just hammers the provider with guaranteed-failing requests.
    provider.push_open_error(ProviderError::Auth("invalid api key".into()));
    // A success turn is queued but must NEVER be reached.
    provider.push_text_turn("should not run");

    let runtime = boot(provider.clone());
    let err = runtime
        .run_turn(turn(SessionId::new()), CancellationToken::new())
        .await
        .expect_err("auth error must not be retried");

    assert!(
        matches!(
            err,
            openlet_core::error::CoreError::Provider(ProviderError::Auth(_))
        ),
        "expected Auth error to bubble, got {err:?}"
    );
    assert_eq!(provider.call_count(), 1, "auth error must not retry");
}

#[tokio::test]
async fn exhausts_attempts_then_surfaces_last_error() {
    let provider = Arc::new(ScriptedProvider::new());
    // More failures than max_attempts (4) → the turn fails after the cap.
    for _ in 0..6 {
        provider.push_open_error(ProviderError::RateLimit { retry_after_ms: 1 });
    }

    let runtime = boot(provider.clone());
    let err = runtime
        .run_turn(turn(SessionId::new()), CancellationToken::new())
        .await
        .expect_err("should fail after exhausting attempts");

    assert!(
        matches!(
            err,
            openlet_core::error::CoreError::Provider(ProviderError::RateLimit { .. })
        ),
        "expected RateLimit to surface, got {err:?}"
    );
    // Capped at max_attempts = 4 opens, no more.
    assert_eq!(provider.call_count(), 4, "must stop at max_attempts");
}
