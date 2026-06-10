//! Parity test: drive the real `OpenAiCompatProvider` against the in-process
//! mock service, assert the streamed `ChatDelta`s match the expected shape.
//!
//! The token `PARITY_SCENARIO:<name>` embedded in the user message picks the
//! canned response — no live network, no API key.

use futures::StreamExt;
use openlet_adapters::openai_compat::OpenAiCompatProvider;
use openlet_core::adapters::model_provider::{ChatDelta, ChatRequest, FinishReason, ModelProvider};
use openlet_core::error::ProviderError;
use openlet_core::projection::{LlmMessage, LlmRole};
use openlet_test_mock_provider::{MockOpenAiService, SCENARIO_PREFIX};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;

fn make_request(scenario: &str) -> ChatRequest {
    ChatRequest {
        model: "test/model".to_string(),
        messages: vec![LlmMessage {
            role: LlmRole::User,
            content: format!("{SCENARIO_PREFIX}{scenario}"),
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

async fn drain(provider: &OpenAiCompatProvider, scenario: &str) -> Vec<ChatDelta> {
    let req = make_request(scenario);
    let mut stream = provider
        .chat_stream(req, CancellationToken::new())
        .await
        .expect("chat_stream open");
    let mut out = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(d) => out.push(d),
            Err(e) => panic!("unexpected stream error: {e:?}"),
        }
    }
    out
}

async fn drain_expect_err(provider: &OpenAiCompatProvider, scenario: &str) -> ProviderError {
    let req = make_request(scenario);
    match provider.chat_stream(req, CancellationToken::new()).await {
        Err(e) => e,
        Ok(mut stream) => {
            while let Some(item) = stream.next().await {
                if let Err(e) = item {
                    return e;
                }
            }
            panic!("expected error, stream completed cleanly");
        }
    }
}

#[tokio::test]
async fn parity_simple_text() {
    let svc = MockOpenAiService::spawn().await.unwrap();
    let provider = OpenAiCompatProvider::new(svc.base_url(), Some(SecretString::from("test-key")));

    let deltas = drain(&provider, "simple_text").await;

    let texts: Vec<&str> = deltas
        .iter()
        .filter_map(|d| match d {
            ChatDelta::Content { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["Hello", ", world"]);

    let finish = deltas.last().expect("at least one delta");
    match finish {
        ChatDelta::Finish {
            reason: FinishReason::EndTurn,
            usage: Some(u),
        } => {
            assert_eq!(u.input_tokens, 12);
            assert_eq!(u.output_tokens, 3);
        }
        other => panic!("expected Finish(EndTurn,Some), got {other:?}"),
    }
}

#[tokio::test]
async fn parity_with_tool_call() {
    let svc = MockOpenAiService::spawn().await.unwrap();
    let provider = OpenAiCompatProvider::new(svc.base_url(), Some(SecretString::from("test-key")));

    let deltas = drain(&provider, "with_tool_call").await;

    // Exactly one ToolCallStart with id+name.
    let starts: Vec<_> = deltas
        .iter()
        .filter_map(|d| match d {
            ChatDelta::ToolCallStart {
                call_id,
                name,
                index,
            } => Some((call_id.as_str(), name.as_str(), *index)),
            _ => None,
        })
        .collect();
    assert_eq!(starts, vec![("call_abc", "bash", 0)]);

    // Args reassemble to the original JSON.
    let args: String = deltas
        .iter()
        .filter_map(|d| match d {
            ChatDelta::ToolCallArgsDelta { args_chunk, .. } => Some(args_chunk.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(args, r#"{"command":"echo hi"}"#);

    // Finish reason maps to ToolUse.
    assert!(matches!(
        deltas.last(),
        Some(ChatDelta::Finish {
            reason: FinishReason::ToolUse,
            ..
        })
    ));
}

#[tokio::test]
async fn parity_reasoning() {
    let svc = MockOpenAiService::spawn().await.unwrap();
    let provider = OpenAiCompatProvider::new(svc.base_url(), Some(SecretString::from("test-key")));

    let deltas = drain(&provider, "reasoning").await;

    assert!(deltas.iter().any(|d| matches!(
        d,
        ChatDelta::Reasoning { text, .. } if text == "thinking..."
    )));
    assert!(
        deltas
            .iter()
            .any(|d| matches!(d, ChatDelta::Content { text } if text == "answer"))
    );
}

#[tokio::test]
async fn parity_rate_limit_maps_to_provider_error() {
    let svc = MockOpenAiService::spawn().await.unwrap();
    let provider = OpenAiCompatProvider::new(svc.base_url(), Some(SecretString::from("test-key")));

    let err = drain_expect_err(&provider, "rate_limit").await;
    assert!(
        matches!(err, ProviderError::RateLimit { .. }),
        "expected RateLimit, got {err:?}"
    );
}

#[tokio::test]
async fn parity_context_overflow_maps_to_network_error() {
    // The current adapter maps non-2xx (other than 401/403/429) to
    // `Network`. This test pins that behavior so we notice if it changes.
    let svc = MockOpenAiService::spawn().await.unwrap();
    let provider = OpenAiCompatProvider::new(svc.base_url(), Some(SecretString::from("test-key")));

    let err = drain_expect_err(&provider, "context_overflow").await;
    assert!(
        matches!(err, ProviderError::Network(_)),
        "expected Network, got {err:?}"
    );
}

#[tokio::test]
async fn captured_request_carries_authorization_header() {
    let svc = MockOpenAiService::spawn().await.unwrap();
    let provider = OpenAiCompatProvider::new(svc.base_url(), Some(SecretString::from("test-key")));

    let _ = drain(&provider, "simple_text").await;

    let captured = svc.captured_requests();
    assert_eq!(captured.len(), 1);
    let req = &captured[0];
    assert_eq!(req.method, "POST");
    assert!(req.path.ends_with("/chat/completions"));
    assert!(
        req.headers
            .iter()
            .any(|(k, v)| k == "authorization" && v == "Bearer test-key"),
        "missing/bad auth header in {:?}",
        req.headers
    );
}
