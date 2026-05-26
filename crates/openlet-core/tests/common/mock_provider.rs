//! `ScriptedProvider` — drives the runtime with a queue of canned
//! `ChatDelta` sequences. Each call to `chat_stream` pops one queued
//! turn off the front of the queue. Cancellation is observed between
//! deltas; if the token trips, the stream emits a synthetic
//! `Finish { reason: Cancelled }` frame instead of the next delta.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::stream::{self, BoxStream, StreamExt};
use openlet_core::adapters::model_provider::{
    ChatDelta, ChatRequest, ChatStream, FinishReason, ModelPricing, ModelProvider,
};
use openlet_core::error::ProviderError;
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
        _req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        *self.call_count.lock().unwrap() += 1;

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
}
