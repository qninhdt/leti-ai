//! Gemini provider — STUB.
//!
//! This module exposes [`GeminiProvider`] as a thin wrapper around the
//! OpenAI-compat client. Real `streamGenerateContent` wire format
//! (`contents[].parts[]`, `inlineData` / `fileData`, `tools.functionCall`)
//! is deferred to a follow-up PR.
//!
//! Today it works because:
//!   - Vertex AI's OpenAI-compat endpoint accepts OpenAI-shape
//!   - OpenRouter accepts `google/*` and translates server-side
//!
//! `capabilities()` claims vision-true for the gemini-1.5 / gemini-2.0
//! families so the runtime can route image-bearing prompts when
//! configured.

use std::sync::Arc;

use async_trait::async_trait;
use openlet_core::adapters::model_provider::{
    CacheHint, ChatRequest, ChatStream, ModelPricing, ModelProvider, ProviderCapabilities,
};
use openlet_core::error::ProviderError;
use openlet_core::projection::LlmMessage;
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;

use crate::openai_compat::OpenAiCompatProvider;
use crate::stub_provider::StubVisionProvider;

/// Default Gemini API base. Stub today; native adapter swaps to
/// `streamGenerateContent` against this base.
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1";

/// Gemini per-image cap: ~20 MB.
const GEMINI_MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// Stub Gemini provider. Delegates to [`OpenAiCompatProvider`] via
/// [`StubVisionProvider`]; flips vision flags for known gemini
/// vision-capable families.
#[derive(Clone)]
pub struct GeminiProvider {
    inner: StubVisionProvider,
}

impl GeminiProvider {
    /// Construct with explicit base URL + key. Use OpenRouter's base
    /// (`https://openrouter.ai/api/v1`) until the native adapter lands.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: Option<SecretString>) -> Self {
        Self::from_openai_compat(Arc::new(OpenAiCompatProvider::new(base_url, api_key)))
    }

    /// Wrap an existing OpenAI-compat client.
    #[must_use]
    pub fn from_openai_compat(inner: Arc<OpenAiCompatProvider>) -> Self {
        Self {
            inner: StubVisionProvider::new(inner, is_vision_model, GEMINI_MAX_IMAGE_BYTES),
        }
    }
}

#[async_trait]
impl ModelProvider for GeminiProvider {
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
        self.inner.capabilities(model)
    }

    fn apply_cache_markers(&self, messages: &mut Vec<LlmMessage>, hint: CacheHint) {
        self.inner.apply_cache_markers(messages, hint);
    }
}

/// Whitelist of Gemini model families that accept images. Strict prefix
/// match so a custom OpenRouter `gemini-myfork/foo` does NOT
/// false-positive — collisions are handled by `MultiProvider` routing,
/// not here.
fn is_vision_model(model: &str) -> bool {
    let m = model.strip_prefix("google/").unwrap_or(model);
    m.starts_with("gemini-1.5-") || m.starts_with("gemini-2.0-") || m.starts_with("gemini-2.5-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vision_capability_flagged_for_known_families() {
        let p = GeminiProvider::new("http://localhost", None);
        assert!(p.capabilities("gemini-1.5-pro").supports_vision);
        assert!(p.capabilities("gemini-2.0-flash").supports_vision);
        assert!(p.capabilities("google/gemini-2.0-flash").supports_vision);
    }

    #[test]
    fn vision_off_for_unknown_or_legacy() {
        let p = GeminiProvider::new("http://localhost", None);
        assert!(!p.capabilities("gemini-pro").supports_vision);
        assert!(!p.capabilities("gemini-1.0").supports_vision);
    }
}
