//! OpenRouter request enrichment is observed ON THE WIRE.
//!
//! The base OpenAI adapter has no concept of attribution headers, a
//! `provider` routing block, or a `models` fallback array. This drives
//! the real `OpenRouterProvider` against the in-process mock and asserts
//! those three enrichments appear in the captured request — and that an
//! empty config sends a vanilla OpenAI-shaped request (no extra keys).
//!
//! `MockOpenAiService.captured_requests()` is the only mock that sees the
//! actual outbound HTTP body + headers, so enrichment must be verified
//! here rather than against a domain-level mock.

use futures::StreamExt;
use leti_adapters::openrouter::{OpenRouterConfig, OpenRouterProvider, ProviderRouting};
use leti_core::adapters::model_provider::{ChatRequest, ModelProvider};
use leti_core::projection::{LlmMessage, LlmRole};
use leti_test_mock_provider::{MockOpenAiService, SCENARIO_PREFIX};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;

fn make_request() -> ChatRequest {
    ChatRequest {
        model: "test/model".to_string(),
        messages: vec![LlmMessage {
            role: LlmRole::User,
            content: format!("{SCENARIO_PREFIX}simple_text"),
            reasoning: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        max_tokens: Some(64),
        temperature: Some(0.0),
        tools: vec![],
        stream: true,
        headers: Default::default(),
    }
}

async fn drive(provider: &OpenRouterProvider) {
    let mut stream = provider
        .chat_stream(make_request(), CancellationToken::new())
        .await
        .expect("chat_stream open");
    while stream.next().await.is_some() {}
}

/// Full config: attribution headers land as `HTTP-Referer` / `X-Title`,
/// the routing block serializes into `provider`, and the fallback list
/// serializes into `models` — all in the single captured request.
#[tokio::test]
async fn enrichment_lands_on_the_wire() {
    let svc = MockOpenAiService::spawn().await.unwrap();
    let config = OpenRouterConfig {
        referer: Some("https://leti.ai".into()),
        title: Some("Leti".into()),
        routing: Some(ProviderRouting {
            order: vec!["Anthropic".into(), "Together".into()],
            allow_fallbacks: Some(false),
            require_parameters: None,
        }),
        models_fallback: vec!["a/primary".into(), "b/backup".into()],
    };
    let provider =
        OpenRouterProvider::new(svc.base_url(), Some(SecretString::from("test-key")), config);

    drive(&provider).await;

    let captured = svc.captured_requests();
    assert_eq!(captured.len(), 1);
    let req = &captured[0];

    // Attribution headers (lowercased by the HTTP layer).
    let header = |name: &str| {
        req.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(header("http-referer"), Some("https://leti.ai"));
    assert_eq!(header("x-title"), Some("Leti"));

    // Body carries the `provider` routing block and `models` fallback.
    let body: serde_json::Value = serde_json::from_str(&req.body).expect("body json");
    assert_eq!(body["provider"]["order"][0], "Anthropic");
    assert_eq!(body["provider"]["order"][1], "Together");
    assert_eq!(body["provider"]["allow_fallbacks"], false);
    // require_parameters was None → omitted, not null.
    assert!(body["provider"].get("require_parameters").is_none());
    assert_eq!(body["models"][0], "a/primary");
    assert_eq!(body["models"][1], "b/backup");
    // Primary `model` still sent for backward compatibility.
    assert_eq!(body["model"], "test/model");
}

/// Empty config → vanilla OpenAI request. No attribution headers, no
/// `provider` block, no `models` array. Proves enrichment is purely
/// additive and OpenRouter accepts a plain OpenAI-shaped body.
#[tokio::test]
async fn empty_config_sends_vanilla_request() {
    let svc = MockOpenAiService::spawn().await.unwrap();
    let provider = OpenRouterProvider::new(
        svc.base_url(),
        Some(SecretString::from("test-key")),
        OpenRouterConfig::default(),
    );

    drive(&provider).await;

    let captured = svc.captured_requests();
    assert_eq!(captured.len(), 1);
    let req = &captured[0];

    assert!(
        !req.headers
            .iter()
            .any(|(k, _)| k == "http-referer" || k == "x-title"),
        "no attribution headers when config is empty: {:?}",
        req.headers
    );
    let body: serde_json::Value = serde_json::from_str(&req.body).expect("body json");
    assert!(
        body.get("provider").is_none(),
        "no provider block when unset"
    );
    assert!(body.get("models").is_none(), "no models array when unset");
}
