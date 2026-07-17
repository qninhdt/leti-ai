//! Wiremock-driven tests for `OpenAiProvider` HTTP error mapping.
//!
//! Locks the contract for `map_http_error`:
//! 1. 401 → `ProviderError::Auth` (with truncated body).
//! 2. 403 → `ProviderError::Auth`.
//! 3. 429 with `Retry-After: <secs>` → `ProviderError::RateLimit`
//!    with `retry_after_ms` derived from the header (× 1000).
//! 4. 429 without `Retry-After` → `ProviderError::RateLimit` with
//!    a fallback `retry_after_ms = 1000`.
//! 5. 5xx → `ProviderError::Network`.
//! 6. 4xx other than 401/403/429 → `ProviderError::Network`.
//! 7. Reserved-header filtering: a plugin trying to set `Authorization`
//!    via `req.headers` is silently dropped (the built-in `Bearer …`
//!    wins).

mod common;

use common::wiremock_helpers::mount_status_only;
use leti_adapters::openai::OpenAiProvider;
use leti_core::adapters::ModelProvider;
use leti_core::adapters::model_provider::{ChatRequest, FinishReason};
use leti_core::error::ProviderError;
use leti_core::projection::{LlmMessage, LlmRole};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn make_request(model: &str) -> ChatRequest {
    ChatRequest {
        model: model.to_string(),
        messages: vec![LlmMessage {
            role: LlmRole::User,
            content: "hi".to_string(),
            reasoning: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        max_tokens: None,
        temperature: None,
        tools: vec![],
        stream: true,
        headers: std::collections::BTreeMap::new(),
    }
}

async fn make_provider(server: &MockServer) -> OpenAiProvider {
    OpenAiProvider::new(
        format!("{}/v1", server.uri()),
        Some(SecretString::new("sk-test".into())),
    )
}

async fn expect_err(provider: &OpenAiProvider, model: &str) -> ProviderError {
    match provider
        .chat_stream(make_request(model), CancellationToken::new())
        .await
    {
        Ok(_) => panic!("expected error, got Ok stream"),
        Err(e) => e,
    }
}

#[tokio::test]
async fn unauthorized_maps_to_auth_error() {
    let server = MockServer::start().await;
    mount_status_only(&server, 401).await;
    let provider = make_provider(&server).await;
    let err = expect_err(&provider, "gpt-5").await;
    assert!(
        matches!(err, ProviderError::Auth(_)),
        "401 must map to Auth, got {err:?}"
    );
}

#[tokio::test]
async fn forbidden_maps_to_auth_error() {
    let server = MockServer::start().await;
    mount_status_only(&server, 403).await;
    let provider = make_provider(&server).await;
    let err = expect_err(&provider, "gpt-5").await;
    assert!(
        matches!(err, ProviderError::Auth(_)),
        "403 must map to Auth, got {err:?}"
    );
}

#[tokio::test]
async fn rate_limit_with_retry_after_carries_header_value() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "7"))
        .mount(&server)
        .await;
    let provider = make_provider(&server).await;
    let err = expect_err(&provider, "gpt-5").await;
    match err {
        ProviderError::RateLimit { retry_after_ms } => {
            assert_eq!(retry_after_ms, 7_000, "retry-after: 7 → 7000ms");
        }
        other => panic!("expected RateLimit, got {other:?}"),
    }
}

#[tokio::test]
async fn rate_limit_without_retry_after_uses_fallback() {
    let server = MockServer::start().await;
    mount_status_only(&server, 429).await;
    let provider = make_provider(&server).await;
    let err = expect_err(&provider, "gpt-5").await;
    match err {
        ProviderError::RateLimit { retry_after_ms } => {
            assert_eq!(
                retry_after_ms, 1_000,
                "missing Retry-After → 1000ms fallback"
            );
        }
        other => panic!("expected RateLimit, got {other:?}"),
    }
}

#[tokio::test]
async fn server_error_maps_to_network_error() {
    let server = MockServer::start().await;
    mount_status_only(&server, 503).await;
    let provider = make_provider(&server).await;
    let err = expect_err(&provider, "gpt-5").await;
    assert!(
        matches!(err, ProviderError::Network(_)),
        "503 must map to Network, got {err:?}"
    );
}

#[tokio::test]
async fn unknown_4xx_maps_to_network_error() {
    let server = MockServer::start().await;
    mount_status_only(&server, 418).await; // I'm a teapot
    let provider = make_provider(&server).await;
    let err = expect_err(&provider, "gpt-5").await;
    assert!(
        matches!(err, ProviderError::Network(_)),
        "418 must map to Network, got {err:?}"
    );
}

#[tokio::test]
async fn missing_api_key_returns_missing_credentials_error_before_request() {
    // Provider with no api key never hits the network — the error
    // surfaces synchronously from chat_stream.
    let provider = OpenAiProvider::new("https://example.invalid/v1".to_string(), None);
    let err = match provider
        .chat_stream(make_request("gpt-5"), CancellationToken::new())
        .await
    {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(
        matches!(err, ProviderError::MissingCredentials { .. }),
        "no api key must surface MissingCredentials, got {err:?}"
    );
}

#[tokio::test]
async fn cancellation_during_request_returns_cancelled() {
    // Wiremock with a delayed response — cancel before it lands.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_delay(std::time::Duration::from_secs(5))
                .set_body_string("data: [DONE]\n\n"),
        )
        .mount(&server)
        .await;
    let provider = make_provider(&server).await;
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel_clone.cancel();
    });
    let err = match provider.chat_stream(make_request("gpt-5"), cancel).await {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    assert!(
        matches!(err, ProviderError::Cancelled),
        "cancellation during request must surface Cancelled, got {err:?}"
    );
}

#[tokio::test]
async fn reserved_header_from_plugin_does_not_override_built_in_authorization() {
    // The provider strips reserved headers (Authorization, x-api-key,
    // etc.) from req.headers structurally. We assert the request still
    // succeeds (and reaches the mock with the right Bearer token) even
    // when a plugin attempts to set its own Authorization.
    let server = MockServer::start().await;
    // Mount a 200 with empty stream; we just need the request to land.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::header(
            "authorization",
            "Bearer sk-test",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(""),
        )
        .mount(&server)
        .await;
    let provider = make_provider(&server).await;
    let mut req = make_request("gpt-5");
    req.headers
        .insert("Authorization".to_string(), "Bearer hijacked".to_string());
    // The mock matcher requires the built-in Bearer; if the plugin
    // header had won, the matcher would 404.
    let res = provider.chat_stream(req, CancellationToken::new()).await;
    // Successful match → empty stream returned (Ok). 404 from a
    // mismatched mock would surface a Network error — locking the
    // assertion negatively guards against the hijack.
    let ok = res.is_ok();
    assert!(ok, "built-in Authorization must win over plugin attempt");
    let _ = FinishReason::EndTurn; // silence unused import
}
