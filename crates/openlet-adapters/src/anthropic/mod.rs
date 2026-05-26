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

/// Default Anthropic Messages API base. Kept for documentation —
/// today's stub uses OpenAI-compat shape so this base will reject
/// requests until the native adapter lands.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com/v1";

/// Stub Anthropic provider. Delegates to [`OpenAiCompatProvider`] for
/// the wire call; overrides `capabilities()` for known vision-capable
/// claude families.
#[derive(Clone)]
pub struct AnthropicProvider {
    inner: Arc<OpenAiCompatProvider>,
}

impl AnthropicProvider {
    /// Construct with explicit base URL + key. Use OpenRouter's base
    /// (`https://openrouter.ai/api/v1`) until the native adapter lands.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: Option<SecretString>) -> Self {
        Self {
            inner: Arc::new(OpenAiCompatProvider::new(base_url, api_key)),
        }
    }

    /// Wrap an existing OpenAI-compat client (e.g. the one built by the
    /// server bootstrap). Useful when integrators already configured
    /// retries / proxies on the inner client.
    #[must_use]
    pub fn from_openai_compat(inner: Arc<OpenAiCompatProvider>) -> Self {
        Self { inner }
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
        let mut caps = self.inner.capabilities(model);
        // Vision: claude sonnet/opus/haiku 3+ all accept images. Older
        // claude-2 / claude-instant did not.
        if is_vision_model(model) {
            caps.supports_vision = true;
            caps.supports_document_input = true;
            // Anthropic per-image cap: ~5 MB before they reject.
            caps.max_image_bytes = 5 * 1024 * 1024;
        }
        caps
    }

    fn apply_cache_markers(&self, _messages: &mut Vec<LlmMessage>, _hint: CacheHint) {
        // STUB: real impl injects `cache_control: {type: "ephemeral"}`
        // into the system + last user turn content blocks. Today's
        // OpenAI-compat shape has no equivalent, so this is a no-op.
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
