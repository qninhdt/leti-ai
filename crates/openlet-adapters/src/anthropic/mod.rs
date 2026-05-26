//! Anthropic provider — STUB.
//!
//! This module exposes [`AnthropicProvider`] as a thin wrapper around
//! the OpenAI-compat client. Real Messages-API wire format
//! (top-level `system`, `content` block array with `cache_control`,
//! `tool_use` / `tool_result` shapes) is deferred to a follow-up PR.
//!
//! Today it works because:
//!   - OpenRouter accepts OpenAI-compat shape for `anthropic/*` and
//!     translates server-side
//!   - `https://api.anthropic.com/v1/...` does NOT accept this shape
//!     directly — integrators pointing at it directly need the native
//!     adapter
//!
//! `capabilities()` claims vision-true for the production claude
//! sonnet / opus / haiku families so the runtime can route image-bearing
//! prompts to this provider when configured.

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

/// Default Anthropic Messages API base. Kept for documentation —
/// today's stub uses OpenAI-compat shape so this base will reject
/// requests until the native adapter lands.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";

/// Anthropic per-image cap: ~5 MB before they reject.
const ANTHROPIC_MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// Stub Anthropic provider. Delegates to [`OpenAiCompatProvider`] via
/// [`StubVisionProvider`]; flips vision flags for known claude
/// vision-capable families.
#[derive(Clone)]
pub struct AnthropicProvider {
    inner: StubVisionProvider,
}

impl AnthropicProvider {
    /// Construct with explicit base URL + key. Use OpenRouter's base
    /// (`https://openrouter.ai/api/v1`) until the native adapter lands.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: Option<SecretString>) -> Self {
        Self::from_openai_compat(Arc::new(OpenAiCompatProvider::new(base_url, api_key)))
    }

    /// Wrap an existing OpenAI-compat client (e.g. the one built by the
    /// server bootstrap). Useful when integrators already configured
    /// retries / proxies on the inner client.
    #[must_use]
    pub fn from_openai_compat(inner: Arc<OpenAiCompatProvider>) -> Self {
        Self {
            inner: StubVisionProvider::new(inner, is_vision_model, ANTHROPIC_MAX_IMAGE_BYTES),
        }
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
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

/// Whitelist of Anthropic model families that accept images. Strict
/// prefix matching on the family root so future variants
/// (`claude-sonnet-5-...`) inherit the flag without a code change.
fn is_vision_model(model: &str) -> bool {
    // Strip `anthropic/` vendor prefix when present.
    let m = model.strip_prefix("anthropic/").unwrap_or(model);
    m.starts_with("claude-sonnet-")
        || m.starts_with("claude-opus-")
        || m.starts_with("claude-haiku-")
        // Legacy versioned names: `claude-3-5-sonnet-...`,
        // `claude-3-opus-...`, `claude-3-haiku-...`.
        || m.starts_with("claude-3-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vision_capability_flagged_for_known_families() {
        let p = AnthropicProvider::new("http://localhost", None);
        assert!(p.capabilities("claude-sonnet-4-5").supports_vision);
        assert!(p.capabilities("claude-opus-4-1").supports_vision);
        assert!(p.capabilities("claude-haiku-4-5").supports_vision);
        assert!(
            p.capabilities("anthropic/claude-sonnet-4-5")
                .supports_vision
        );
        assert!(p.capabilities("claude-3-5-sonnet-latest").supports_vision);
    }

    #[test]
    fn vision_off_for_legacy_claude2() {
        let p = AnthropicProvider::new("http://localhost", None);
        assert!(!p.capabilities("claude-2").supports_vision);
        assert!(!p.capabilities("claude-instant-1").supports_vision);
    }
}
