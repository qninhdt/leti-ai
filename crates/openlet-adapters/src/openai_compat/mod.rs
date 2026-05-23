//! OpenAI-compat / OpenRouter `ModelProvider` impl.
//!
//! Phase 1 stub. Phase 3 fills in `chat_stream` against OpenRouter's
//! standard OpenAI-compat endpoint.

use async_trait::async_trait;
use futures::Stream;
use openlet_core::adapters::model_provider::{ChatDelta, ChatRequest, ModelPricing, ModelProvider};
use openlet_core::error::ProviderError;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
pub struct OpenAiCompatProvider;

impl OpenAiCompatProvider {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatProvider {
    async fn chat_stream(
        &self,
        _req: ChatRequest,
        _cancel: CancellationToken,
    ) -> Result<
        Box<dyn Stream<Item = Result<ChatDelta, ProviderError>> + Send + Unpin>,
        ProviderError,
    > {
        Err(ProviderError::Unimplemented)
    }

    fn pricing(&self, _model: &str) -> Option<ModelPricing> {
        None
    }
}
