//! Shared stub-provider scaffold for vision-augmented OpenAI-compat
//! delegates.
//!
//! [`AnthropicProvider`] and [`GeminiProvider`] are both thin wrappers
//! around [`OpenAiCompatProvider`] that override `capabilities()` to
//! flip `supports_vision`/`supports_document_input` + bump
//! `max_image_bytes` for known vision-capable model families. The wire
//! call, pricing, and (no-op) cache-marker logic are identical.
//!
//! [`StubVisionProvider`] hoists that shared shape into one place.
//! Each provider becomes a thin `pub struct Foo(StubVisionProvider)`
//! constructor + a `vision_check` fn pointer + a `max_image_bytes`
//! constant. ~150 lines of trait-impl boilerplate go away.
//!
//! [`AnthropicProvider`]: crate::anthropic::AnthropicProvider
//! [`GeminiProvider`]: crate::gemini::GeminiProvider
//! [`OpenAiCompatProvider`]: crate::openai_compat::OpenAiCompatProvider

use std::sync::Arc;

use async_trait::async_trait;
use openlet_core::adapters::model_provider::{
    CacheHint, ChatRequest, ChatStream, ModelPricing, ModelProvider, ProviderCapabilities,
};
use openlet_core::error::ProviderError;
use openlet_core::projection::LlmMessage;
use tokio_util::sync::CancellationToken;

use crate::openai_compat::OpenAiCompatProvider;

/// `model -> bool` predicate identifying vision-capable model families
/// for a given provider. Each stub provider supplies its own.
pub type VisionCheck = fn(&str) -> bool;

/// Generic vision-augmented delegate. The wire call goes to `inner`;
/// `capabilities()` flips vision flags + caps for any model where
/// `vision_check` returns `true`.
#[derive(Clone)]
pub struct StubVisionProvider {
    inner: Arc<OpenAiCompatProvider>,
    vision_check: VisionCheck,
    max_image_bytes: usize,
}

impl StubVisionProvider {
    /// Construct from an existing OpenAI-compat client.
    #[must_use]
    pub fn new(
        inner: Arc<OpenAiCompatProvider>,
        vision_check: VisionCheck,
        max_image_bytes: usize,
    ) -> Self {
        Self {
            inner,
            vision_check,
            max_image_bytes,
        }
    }
}

#[async_trait]
impl ModelProvider for StubVisionProvider {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        self.inner.chat_stream(req, cancel).await
    }

    fn pricing(&self, model: &str) -> Option<ModelPricing> {
        self.inner.pricing(model)
    }

    fn capabilities(&self, model: &str) -> ProviderCapabilities {
        let mut caps = self.inner.capabilities(model);
        if (self.vision_check)(model) {
            caps.supports_vision = true;
            caps.supports_document_input = true;
            caps.max_image_bytes = self.max_image_bytes;
        }
        caps
    }

    fn apply_cache_markers(&self, _messages: &mut Vec<LlmMessage>, _hint: CacheHint) {
        // Stubs no-op cache markers — Anthropic-native shape (cache_control
        // blocks) and Gemini-native (cachedContent refs) require the
        // real wire format adapters. Today's OpenAI-compat shape has no
        // place to put them.
    }
}
