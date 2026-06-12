//! `OpenRouterProvider` — the OpenAI base adapter plus OpenRouter
//! request enrichment (attribution headers, provider routing, model
//! fallback). Transport, wire serialization, prefix-shaping, and pricing
//! are reused verbatim from [`crate::openai`].

use async_trait::async_trait;
use reqwest::Client;
use reqwest::header::{HeaderName, HeaderValue};
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;

use openlet_core::adapters::model_provider::{
    ChatRequest, ChatStream, ModelInfo, ModelPricing, ModelProvider, ProviderCapabilities,
};
use openlet_core::error::ProviderError;

use crate::openai::provider::build_chat_body;
use crate::openai::shared_provider::{shared_capabilities, shared_list_models, shared_pricing};
use crate::openai::transport::HttpTransport;

use super::config::OpenRouterConfig;

/// OpenRouter API base. Override only for a self-hosted OpenRouter
/// gateway or testing against `wiremock`.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

const HTTP_REFERER: HeaderName = HeaderName::from_static("http-referer");
const X_TITLE: HeaderName = HeaderName::from_static("x-title");

#[derive(Clone, Debug)]
pub struct OpenRouterProvider {
    transport: HttpTransport,
    config: OpenRouterConfig,
}

impl OpenRouterProvider {
    /// Build with explicit base URL + key + OpenRouter config.
    #[must_use]
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<SecretString>,
        config: OpenRouterConfig,
    ) -> Self {
        Self {
            transport: HttpTransport::new(base_url, api_key, "openrouter", "OPENROUTER_API_KEY"),
            config,
        }
    }

    /// Reuse a caller-built `reqwest::Client` (shared pool).
    #[must_use]
    pub fn from_parts(
        base_url: String,
        api_key: Option<SecretString>,
        http: Client,
        config: OpenRouterConfig,
    ) -> Self {
        Self {
            transport: HttpTransport::from_parts(
                base_url,
                api_key,
                http,
                "openrouter",
                "OPENROUTER_API_KEY",
            ),
            config,
        }
    }

    /// Attribution headers (`HTTP-Referer`, `X-Title`). Built once per
    /// request; both are non-secret app metadata.
    fn attribution_headers(&self) -> Vec<(HeaderName, HeaderValue)> {
        let mut out = Vec::new();
        if let Some(referer) = self.config.referer.as_deref() {
            if let Ok(v) = HeaderValue::from_str(referer) {
                out.push((HTTP_REFERER, v));
            }
        }
        if let Some(title) = self.config.title.as_deref() {
            if let Ok(v) = HeaderValue::from_str(title) {
                out.push((X_TITLE, v));
            }
        }
        out
    }

    /// Enrich the base OpenAI body in place with OpenRouter fields:
    /// `provider` routing block + `models` fallback array.
    fn enrich_body(&self, body: &mut serde_json::Value) {
        let Some(obj) = body.as_object_mut() else {
            return;
        };
        if let Some(routing) = self.config.routing.as_ref() {
            if !routing.is_empty() {
                if let Ok(v) = serde_json::to_value(routing) {
                    obj.insert("provider".to_string(), v);
                }
            }
        }
        if !self.config.models_fallback.is_empty() {
            obj.insert(
                "models".to_string(),
                serde_json::Value::from(self.config.models_fallback.clone()),
            );
        }
    }
}

impl Default for OpenRouterProvider {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL, None, OpenRouterConfig::default())
    }
}

#[async_trait]
impl ModelProvider for OpenRouterProvider {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        let mut body = build_chat_body(&req)?;
        self.enrich_body(&mut body);
        let attribution = self.attribution_headers();
        self.transport
            .post_chat_stream(&body, &req.headers, &attribution, cancel)
            .await
    }

    fn pricing(&self, model: &str) -> Option<ModelPricing> {
        shared_pricing(model)
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        shared_list_models(&self.transport).await
    }

    fn capabilities(&self, model: &str) -> ProviderCapabilities {
        shared_capabilities(model)
    }
}
