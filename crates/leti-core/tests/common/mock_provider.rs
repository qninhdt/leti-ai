//! `ScriptedProvider` — drives the runtime with a queue of canned
//! `ChatDelta` sequences. Each call to `chat_stream` pops one queued
//! turn off the front of the queue. Cancellation is observed between
//! deltas; if the token trips, the stream emits a synthetic
//! `Finish { reason: Cancelled }` frame instead of the next delta.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use leti_core::adapters::model_provider::{
    ChatDelta, ChatRequest, ChatStream, FinishReason, ModelPricing, ModelProvider,
    ProviderCapabilities,
};
use leti_core::error::ProviderError;
use tokio_util::sync::CancellationToken;

/// One scripted turn — deltas to emit on the next `chat_stream` call.
pub type ScriptedTurn = Vec<Result<ChatDelta, ProviderError>>;

/// Scriptable `ModelProvider` for tests.
///
/// Set up via `push_turn` (one queue entry per upcoming model call) or
/// `push_raw_stream` for hand-rolled streams (used by streaming
/// back-pressure tests).
pub struct ScriptedProvider {
    turns: Mutex<VecDeque<ScriptedTurn>>,
    raw: Mutex<VecDeque<BoxStream<'static, Result<ChatDelta, ProviderError>>>>,
    pricing: Mutex<Option<ModelPricing>>,
    /// Queue of errors to fail the `chat_stream` OPEN with, popped one per
    /// call before any queued deltas are served. Lets retry/backoff tests
    /// script `429 → 429 → Ok` at the stream-open boundary (distinct from
    /// `push_turn`, which fails mid-stream as a delta). Empty = open
    /// always succeeds.
    open_errors: Mutex<VecDeque<ProviderError>>,
    /// Captures the `model` field of every `ChatRequest` the runtime sends,
    /// in call order. Lets per-session-model tests assert the override
    /// reached the provider.
    seen_models: Arc<Mutex<Vec<String>>>,
    /// Substring marking a model as vision-capable. When set, `capabilities`
    /// returns `supports_vision = true` for models containing it. Lets the
    /// per-session-model test assert capabilities are computed for the
    /// resolved model, not a hardcoded default.
    vision_marker: Mutex<Option<String>>,
    /// Tracks the number of `chat_stream` calls received — useful when
    /// asserting the runtime didn't double-call the provider.
    call_count: Arc<Mutex<usize>>,
}

impl ScriptedProvider {
    #[must_use]
    pub fn new() -> Self {
        Self {
            turns: Mutex::new(VecDeque::new()),
            raw: Mutex::new(VecDeque::new()),
            pricing: Mutex::new(None),
            open_errors: Mutex::new(VecDeque::new()),
            seen_models: Arc::new(Mutex::new(Vec::new())),
            vision_marker: Mutex::new(None),
            call_count: Arc::new(Mutex::new(0)),
        }
    }

    /// Queue a turn worth of deltas. Each call to `chat_stream` pops the
    /// front entry and emits its deltas in order.
    pub fn push_turn(&self, deltas: ScriptedTurn) -> &Self {
        self.turns.lock().unwrap().push_back(deltas);
        self
    }

    /// Convenience: queue a single-shot text-then-EndTurn turn.
    pub fn push_text_turn(&self, text: impl Into<String>) -> &Self {
        let text = text.into();
        self.push_turn(vec![
            Ok(ChatDelta::Role),
            Ok(ChatDelta::Content { text }),
            Ok(ChatDelta::Finish {
                reason: FinishReason::EndTurn,
                usage: None,
            }),
        ])
    }

    /// Queue a raw boxed stream — used for hand-rolled back-pressure /
    /// timing tests where the queue model is too rigid.
    pub fn push_raw_stream(
        &self,
        s: BoxStream<'static, Result<ChatDelta, ProviderError>>,
    ) -> &Self {
        self.raw.lock().unwrap().push_back(s);
        self
    }

    /// Override pricing (default `None`).
    pub fn with_pricing(self, pricing: ModelPricing) -> Self {
        *self.pricing.lock().unwrap() = Some(pricing);
        self
    }

    /// Read the number of `chat_stream` calls the runtime made so far.
    #[must_use]
    pub fn call_count(&self) -> usize {
        *self.call_count.lock().unwrap()
    }

    /// Queue an error to fail the next `chat_stream` OPEN with. Popped one
    /// per call before any queued deltas serve, so `429 → 429 → Ok` scripts
    /// the runtime's retry/backoff at the stream-open boundary.
    pub fn push_open_error(&self, err: ProviderError) -> &Self {
        self.open_errors.lock().unwrap().push_back(err);
        self
    }

    /// The `model` field of every `ChatRequest` received, in call order.
    #[must_use]
    pub fn seen_models(&self) -> Vec<String> {
        self.seen_models.lock().unwrap().clone()
    }

    /// Mark models whose name contains `marker` as vision-capable, so
    /// `capabilities` returns `supports_vision = true` for them. Lets a
    /// per-session-model test assert capabilities track the resolved model.
    pub fn with_vision_marker(self, marker: impl Into<String>) -> Self {
        *self.vision_marker.lock().unwrap() = Some(marker.into());
        self
    }
}

impl Default for ScriptedProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for ScriptedProvider {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        *self.call_count.lock().unwrap() += 1;
        self.seen_models.lock().unwrap().push(req.model.clone());

        // Fail the open with a scripted error if one is queued — drives
        // the runtime's retry/backoff before any stream is produced.
        if let Some(err) = self.open_errors.lock().unwrap().pop_front() {
            return Err(err);
        }

        if let Some(s) = self.raw.lock().unwrap().pop_front() {
            // Raw streams ignore cancellation peeking; callers that
            // care wire it themselves. `s` is a `BoxStream` (Send +
            // Unpin); wrapping in Box gives us the canonical
            // `ChatStream` smart-pointer shape.
            return Ok(Box::new(s) as ChatStream);
        }

        let deltas = self.turns.lock().unwrap().pop_front().unwrap_or_default();

        // Wrap the queued deltas in a stream that peeks `cancel` between
        // emissions. As soon as cancellation trips, replace the rest of
        // the queue with a single synthetic Cancelled finish frame.
        let stream = stream::unfold(
            (deltas.into_iter(), cancel, false),
            |(mut iter, cancel, sent_cancel)| async move {
                if sent_cancel {
                    return None;
                }
                if cancel.is_cancelled() {
                    let frame = Ok(ChatDelta::Finish {
                        reason: FinishReason::Cancelled,
                        usage: None,
                    });
                    return Some((frame, (iter, cancel, true)));
                }
                iter.next().map(|d| (d, (iter, cancel, sent_cancel)))
            },
        );
        Ok(Box::new(stream.boxed()) as ChatStream)
    }

    fn pricing(&self, _model: &str) -> Option<ModelPricing> {
        self.pricing.lock().unwrap().clone()
    }

    fn capabilities(&self, model: &str) -> ProviderCapabilities {
        let mut caps = ProviderCapabilities::default();
        if let Some(marker) = self.vision_marker.lock().unwrap().as_deref() {
            caps.supports_vision = model.contains(marker);
        }
        caps
    }
}
