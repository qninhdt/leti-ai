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

/// Default Gemini API base. Stub today; native adapter swaps to
/// `streamGenerateContent` against this base.
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1";

/// Stub Gemini provider. Delegates to [`OpenAiCompatProvider`] for the
/// wire call; overrides `capabilities()` for known vision-capable
/// gemini families.
#[derive(Clone)]
pub struct GeminiProvider {
    inner: Arc<OpenAiCompatProvider>,
}

impl GeminiProvider {
    /// Construct with explicit base URL + key. Use OpenRouter's base
    /// (`https://openrouter.ai/api/v1`) until the native adapter lands.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: Option<SecretString>) -> Self {
        Self {
            inner: Arc::new(OpenAiCompatProvider::new(base_url, api_key)),
        }
    }

    /// Wrap an existing OpenAI-compat client.
    #[must_use]
    pub fn from_openai_compat(inner: Arc<OpenAiCompatProvider>) -> Self {
        Self { inner }
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
        let mut caps = self.inner.capabilities(model);
        if is_vision_model(model) {
            caps.supports_vision = true;
            caps.supports_document_input = true;
            // Gemini per-image cap: ~20 MB.
            caps.max_image_bytes = 20 * 1024 * 1024;
        }
        caps
    }

    fn apply_cache_markers(&self, _messages: &mut Vec<LlmMessage>, _hint: CacheHint) {
        // STUB: Gemini auto-caches; native adapter would also expose
        // explicit `cachedContent` references for cross-turn reuse.
        // Today no-op — auto-caching covers the common path.
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
