//! OpenRouter-specific request configuration.
//!
//! These knobs map to OpenRouter request enrichment that the generic
//! OpenAI adapter has no concept of. All optional — an unset config
//! sends a vanilla OpenAI-shaped request (still valid for OpenRouter).

use serde::Serialize;

/// OpenRouter app attribution + routing config. Built once at provider
/// construction from env/config and applied to every request.
#[derive(Debug, Clone, Default)]
pub struct OpenRouterConfig {
    /// `HTTP-Referer` header — the app URL OpenRouter shows on its
    /// activity/leaderboard pages. Non-secret.
    pub referer: Option<String>,
    /// `X-Title` header — the app name on OpenRouter's dashboards.
    /// Non-secret.
    pub title: Option<String>,
    /// Provider routing preferences (`provider` block in the body).
    pub routing: Option<ProviderRouting>,
    /// Ordered model fallback list (`models` array). When set, OpenRouter
    /// tries each in order if the primary is unavailable. The primary
    /// `model` field is still sent for backward compatibility.
    pub models_fallback: Vec<String>,
}

impl OpenRouterConfig {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.referer.is_none()
            && self.title.is_none()
            && self.routing.is_none()
            && self.models_fallback.is_empty()
    }
}

/// OpenRouter `provider` routing block. Serialized straight into the
/// request body. Only set fields are sent.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProviderRouting {
    /// Explicit provider priority order (e.g. `["Anthropic", "Together"]`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    /// Whether OpenRouter may fall back to other providers when the
    /// preferred ones are unavailable. `None` = OpenRouter default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_fallbacks: Option<bool>,
    /// Require all routed providers to support every request parameter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub require_parameters: Option<bool>,
}

impl ProviderRouting {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty() && self.allow_fallbacks.is_none() && self.require_parameters.is_none()
    }
}
