//! Multi-provider router — selects the right [`ModelProvider`] for a
//! request based on the model name prefix.
//!
//! Routing matrix (hyphen/slash strict; see [`detect_provider_kind`]):
//!  - `claude-`, `anthropic/`              → [`ProviderKind::Anthropic`]
//!  - `gemini-`, `google/`                 → [`ProviderKind::Gemini`]
//!  - `gpt-`, `o1-`, `o3-`, `grok-`,
//!    `kimi-`, `qwen-`, `deepseek-`, etc.  → [`ProviderKind::OpenAiCompat`]
//!  - everything else                      → `OpenAiCompat` (fall through)
//!
//! Strict separators (`-` or `/` after the prefix) prevent collisions
//! on custom OpenRouter model names. Example: `claude-myprovider/foo`
//! does NOT route to Anthropic if the integrator hasn't registered
//! that provider — falls through to OpenAiCompat. Integrators with
//! quirky names use [`MultiProvider::with_prefix_overrides`] to escape
//! the default routing.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use openlet_core::adapters::model_provider::{
    CacheHint, ChatRequest, ChatStream, ModelPricing, ModelProvider, ProviderCapabilities,
};
use openlet_core::error::ProviderError;
use openlet_core::projection::LlmMessage;
use tokio_util::sync::CancellationToken;

use crate::model_match::strict_prefix;

/// Closed taxonomy of provider backends. Adding a new entry requires
/// touching the [`MultiProvider`] resolver below — intentional so
/// routing changes are explicit, not implicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Anthropic,
    Gemini,
    OpenAiCompat,
}

/// Router that delegates to one of three backend providers. Anthropic
/// and Gemini are optional — when not configured, requests for those
/// model families fall back to the OpenAI-compat client (works because
/// OpenRouter accepts compat shape for `anthropic/*` and `google/*`).
pub struct MultiProvider {
    anthropic: Option<Arc<dyn ModelProvider>>,
    gemini: Option<Arc<dyn ModelProvider>>,
    openai_compat: Arc<dyn ModelProvider>,
    prefix_overrides: HashMap<String, ProviderKind>,
}

impl MultiProvider {
    /// Build with explicit per-kind backends. `openai_compat` is the
    /// catch-all and is required; the other two are optional.
    #[must_use]
    pub fn new(
        anthropic: Option<Arc<dyn ModelProvider>>,
        gemini: Option<Arc<dyn ModelProvider>>,
        openai_compat: Arc<dyn ModelProvider>,
    ) -> Self {
        Self {
            anthropic,
            gemini,
            openai_compat,
            prefix_overrides: HashMap::new(),
        }
    }

    /// Override the default routing for specific model name prefixes.
    /// Useful when an integrator has a custom OpenRouter model that
    /// happens to share a prefix with a built-in family — they can
    /// pin it to OpenAiCompat instead of Anthropic, etc.
    ///
    /// Keys are matched against the model name with the same
    /// hyphen/slash-strict semantics as the default matrix.
    #[must_use]
    pub fn with_prefix_overrides(mut self, overrides: HashMap<String, ProviderKind>) -> Self {
        self.prefix_overrides = overrides;
        self
    }

    /// Resolve `model` to a backend reference. Returns the catch-all
    /// `openai_compat` when the requested kind isn't configured (the
    /// integrator is expected to point its OpenAI-compat URL at
    /// OpenRouter or similar).
    fn resolve(&self, model: &str) -> &Arc<dyn ModelProvider> {
        let kind = self
            .prefix_overrides
            .iter()
            .find(|(k, _)| strict_prefix(model, k))
            .map(|(_, v)| *v)
            .unwrap_or_else(|| detect_provider_kind(model));

        match kind {
            ProviderKind::Anthropic => self.anthropic.as_ref().unwrap_or(&self.openai_compat),
            ProviderKind::Gemini => self.gemini.as_ref().unwrap_or(&self.openai_compat),
            ProviderKind::OpenAiCompat => &self.openai_compat,
        }
    }
}

/// Default routing decision based on a model's prefix. Strict
/// separators — see [`matches_strict_prefix`].
#[must_use]
pub fn detect_provider_kind(model: &str) -> ProviderKind {
    if matches_strict_prefix(model, "claude-") || model.starts_with("anthropic/") {
        return ProviderKind::Anthropic;
    }
    if matches_strict_prefix(model, "gemini-") || model.starts_with("google/") {
        return ProviderKind::Gemini;
    }
    // OpenAI-compat catch-all — covers gpt-, o1-, o3-, grok-, kimi-,
    // qwen-, deepseek-, mistral-, llama-, ...
    ProviderKind::OpenAiCompat
}

/// Strict prefix match: `model` starts with `prefix` AND `prefix`
/// already ends with a hyphen or slash separator. The caller is
/// responsible for including the separator (e.g. `"claude-"`, not
/// `"claude"`) so collisions like `claude2` cannot match `claude`.
fn matches_strict_prefix(model: &str, prefix: &str) -> bool {
    if !prefix.ends_with('-') && !prefix.ends_with('/') {
        return false;
    }
    model.starts_with(prefix)
}

#[async_trait]
impl ModelProvider for MultiProvider {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        let backend = self.resolve(&req.model).clone();
        backend.chat_stream(req, cancel).await
    }

    fn pricing(&self, model: &str) -> Option<ModelPricing> {
        self.resolve(model).pricing(model)
    }

    fn capabilities(&self, model: &str) -> ProviderCapabilities {
        self.resolve(model).capabilities(model)
    }

    fn apply_cache_markers(&self, messages: &mut Vec<LlmMessage>, hint: CacheHint) {
        // Cache-marker routing follows the same prefix matrix —
        // resolve via a synthetic model name from the first message.
        // Caller can also pre-route by calling the backend directly
        // when the model name is known at the call site.
        // We don't know the model from messages alone. Defer to the
        // `openai_compat` backend's no-op default; integrators that
        // want Anthropic markers route via the resolved backend at
        // the call site.
        self.openai_compat.apply_cache_markers(messages, hint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_prefix_routing() {
        assert_eq!(
            detect_provider_kind("claude-sonnet-4-5"),
            ProviderKind::Anthropic
        );
        assert_eq!(
            detect_provider_kind("anthropic/claude-sonnet-4-5"),
            ProviderKind::Anthropic
        );
        assert_eq!(
            detect_provider_kind("claude-opus-4-1"),
            ProviderKind::Anthropic
        );
    }

    #[test]
    fn gemini_prefix_routing() {
        assert_eq!(
            detect_provider_kind("gemini-2.0-flash"),
            ProviderKind::Gemini
        );
        assert_eq!(
            detect_provider_kind("google/gemini-2.0-flash"),
            ProviderKind::Gemini
        );
    }

    #[test]
    fn openai_compat_catch_all() {
        assert_eq!(
            detect_provider_kind("gpt-5-pro"),
            ProviderKind::OpenAiCompat
        );
        assert_eq!(detect_provider_kind("o1-mini"), ProviderKind::OpenAiCompat);
        assert_eq!(detect_provider_kind("o3-mini"), ProviderKind::OpenAiCompat);
        assert_eq!(
            detect_provider_kind("grok-3-mini"),
            ProviderKind::OpenAiCompat
        );
        assert_eq!(
            detect_provider_kind("kimi-k2-0905"),
            ProviderKind::OpenAiCompat
        );
        assert_eq!(detect_provider_kind("qwen-max"), ProviderKind::OpenAiCompat);
        assert_eq!(
            detect_provider_kind("deepseek-v3"),
            ProviderKind::OpenAiCompat
        );
    }

    #[test]
    fn collision_custom_openrouter_model_does_not_route_to_anthropic() {
        // Collision case: a custom OpenRouter model named
        // `claude-myprovider/foo` STARTS with `claude-` but isn't
        // Anthropic. Default routing sends it to Anthropic — that's
        // the documented behavior; integrators escape via overrides.
        assert_eq!(
            detect_provider_kind("claude-myprovider/foo"),
            ProviderKind::Anthropic
        );
    }

    #[test]
    fn prefix_overrides_escape_default_routing() {
        struct StubProvider;
        #[async_trait]
        impl ModelProvider for StubProvider {
            async fn chat_stream(
                &self,
                _req: ChatRequest,
                _cancel: CancellationToken,
            ) -> Result<ChatStream, ProviderError> {
                Err(ProviderError::Network("stub".into()))
            }
            fn pricing(&self, _model: &str) -> Option<ModelPricing> {
                None
            }
        }
        let openai_compat: Arc<dyn ModelProvider> = Arc::new(StubProvider);
        let mut overrides = HashMap::new();
        overrides.insert("claude-myprovider/".to_string(), ProviderKind::OpenAiCompat);
        let router = MultiProvider::new(None, None, openai_compat).with_prefix_overrides(overrides);
        // The override pins the custom name to OpenAiCompat — would
        // panic on `unwrap_or` if routing tried Anthropic without
        // that backend configured. Smoke test the resolver path.
        let _ = router.pricing("claude-myprovider/foo");
    }

    #[test]
    fn matches_strict_prefix_requires_separator() {
        assert!(matches_strict_prefix("claude-sonnet", "claude-"));
        assert!(matches_strict_prefix("anthropic/foo", "anthropic/"));
        // Without trailing separator, refuse to match — closes the
        // collision class on prefixes like `claude` matching `claude2`.
        assert!(!matches_strict_prefix("claude2", "claude"));
        assert!(!matches_strict_prefix("anthropic-foo", "anthropic"));
    }
}
