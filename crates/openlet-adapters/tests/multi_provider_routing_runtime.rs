//! `MultiProvider` routing under concurrent dispatch.
//!
//! Three scripted backends record every call; the test fires N
//! concurrent `chat_stream`s mixing all three model families plus a
//! `prefix_overrides` custom name. Asserts:
//! 1. Each request lands at the right backend (no shared-state bug).
//! 2. `prefix_overrides` wins over default detection.
//! 3. Concurrent dispatches across families never cross-route.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::stream;
use openlet_adapters::multi_provider::{MultiProvider, ProviderKind};
use openlet_core::adapters::model_provider::{
    ChatDelta, ChatRequest, ChatStream, FinishReason, ModelPricing, ModelProvider,
};
use openlet_core::error::ProviderError;
use openlet_core::projection::{LlmMessage, LlmRole};
use tokio_util::sync::CancellationToken;

/// Backend that bumps a counter on every `chat_stream` call so the
/// test can assert which provider received the routed request.
struct CountingBackend {
    label: &'static str,
    calls: Arc<AtomicUsize>,
    last_model: Arc<std::sync::Mutex<Option<String>>>,
}

impl CountingBackend {
    fn new(label: &'static str) -> Self {
        Self {
            label,
            calls: Arc::new(AtomicUsize::new(0)),
            last_model: Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

#[async_trait]
impl ModelProvider for CountingBackend {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        _cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.last_model.lock().unwrap() = Some(req.model.clone());
        let _ = self.label;
        let frames = vec![
            Ok(ChatDelta::Role),
            Ok(ChatDelta::Finish {
                reason: FinishReason::EndTurn,
                usage: None,
            }),
        ];
        Ok(Box::new(stream::iter(frames)))
    }
    fn pricing(&self, _model: &str) -> Option<ModelPricing> {
        None
    }
}

fn make_request(model: &str) -> ChatRequest {
    ChatRequest {
        model: model.to_string(),
        messages: vec![LlmMessage {
            role: LlmRole::User,
            content: "hi".into(),
            reasoning: None,
            tool_calls: vec![],
            tool_call_id: None,
        }],
        system: None,
        max_tokens: None,
        temperature: None,
        tools: vec![],
        stream: true,
        headers: Default::default(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn family_specific_models_route_to_their_own_backend() {
    let anthropic = Arc::new(CountingBackend::new("anthropic"));
    let gemini = Arc::new(CountingBackend::new("gemini"));
    let openai = Arc::new(CountingBackend::new("openai"));

    let router = MultiProvider::new(
        Some(anthropic.clone() as Arc<dyn ModelProvider>),
        Some(gemini.clone() as Arc<dyn ModelProvider>),
        openai.clone() as Arc<dyn ModelProvider>,
    );

    // Drain each request's stream — the simple frames take a single
    // poll so we don't need real iteration here.
    let _ = router
        .chat_stream(make_request("claude-sonnet-4-5"), CancellationToken::new())
        .await
        .unwrap();
    let _ = router
        .chat_stream(make_request("gemini-2.0-flash"), CancellationToken::new())
        .await
        .unwrap();
    let _ = router
        .chat_stream(make_request("gpt-5-pro"), CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(anthropic.calls.load(Ordering::SeqCst), 1);
    assert_eq!(gemini.calls.load(Ordering::SeqCst), 1);
    assert_eq!(openai.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn prefix_override_pins_custom_model_to_openai_compat() {
    let anthropic = Arc::new(CountingBackend::new("anthropic"));
    let openai = Arc::new(CountingBackend::new("openai"));

    let mut overrides = std::collections::HashMap::new();
    // Pin the custom name `claude-myprovider/` to OpenAiCompat so it
    // doesn't route to Anthropic by default.
    overrides.insert("claude-myprovider/".to_string(), ProviderKind::OpenAiCompat);

    let router = MultiProvider::new(
        Some(anthropic.clone() as Arc<dyn ModelProvider>),
        None,
        openai.clone() as Arc<dyn ModelProvider>,
    )
    .with_prefix_overrides(overrides);

    let _ = router
        .chat_stream(
            make_request("claude-myprovider/foo"),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(
        anthropic.calls.load(Ordering::SeqCst),
        0,
        "prefix override must keep Anthropic out of the routing path"
    );
    assert_eq!(
        openai.calls.load(Ordering::SeqCst),
        1,
        "prefix override pinned the custom name to openai-compat"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_dispatch_never_cross_routes() {
    const ITERS: usize = 100;

    let anthropic = Arc::new(CountingBackend::new("anthropic"));
    let gemini = Arc::new(CountingBackend::new("gemini"));
    let openai = Arc::new(CountingBackend::new("openai"));

    let router = Arc::new(MultiProvider::new(
        Some(anthropic.clone() as Arc<dyn ModelProvider>),
        Some(gemini.clone() as Arc<dyn ModelProvider>),
        openai.clone() as Arc<dyn ModelProvider>,
    ));

    // Spawn 3*ITERS concurrent calls, one per family per iteration.
    let mut handles = Vec::with_capacity(3 * ITERS);
    for _ in 0..ITERS {
        for model in ["claude-sonnet-4-5", "gemini-2.0-flash", "gpt-5-pro"] {
            let router = Arc::clone(&router);
            handles.push(tokio::spawn(async move {
                router
                    .chat_stream(make_request(model), CancellationToken::new())
                    .await
                    .map(|_| ())
            }));
        }
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }

    // Each backend received exactly ITERS calls — no cross-routing.
    assert_eq!(anthropic.calls.load(Ordering::SeqCst), ITERS);
    assert_eq!(gemini.calls.load(Ordering::SeqCst), ITERS);
    assert_eq!(openai.calls.load(Ordering::SeqCst), ITERS);
}
