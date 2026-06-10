//! `OpenAiProvider` — base `ModelProvider` for any OpenAI-compatible
//! `POST /v1/chat/completions` endpoint. Pure transport (no retry —
//! that lives in the runtime). The `openrouter` adapter wraps the same
//! [`HttpTransport`] and enriches the request body/headers.

use async_trait::async_trait;
use reqwest::Client;
use secrecy::SecretString;
use tokio_util::sync::CancellationToken;

use openlet_core::adapters::model_provider::{
    ChatRequest, ChatStream, ModelInfo, ModelPricing, ModelProvider, ProviderCapabilities,
};
use openlet_core::error::ProviderError;

use super::prefix_shaping::{apply_request_shaping, detect_quirks};
use super::pricing::pricing_for;
use super::transport::HttpTransport;
use super::wire::to_wire;

/// Default base URL. OpenRouter speaks the OpenAI dialect, so it is a
/// sensible catch-all default even for the base adapter; point at any
/// OpenAI-compatible gateway via `new` / `from_parts`.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

#[derive(Clone, Debug)]
pub struct OpenAiProvider {
    transport: HttpTransport,
}

impl OpenAiProvider {
    /// Build with explicit configuration.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: Option<SecretString>) -> Self {
        Self {
            transport: HttpTransport::new(base_url, api_key, "openai", "OPENAI_API_KEY"),
        }
    }

    /// Reuse a caller-built `reqwest::Client` (shared pool).
    #[must_use]
    pub fn from_parts(base_url: String, api_key: Option<SecretString>, http: Client) -> Self {
        Self {
            transport: HttpTransport::from_parts(
                base_url,
                api_key,
                http,
                "openai",
                "OPENAI_API_KEY",
            ),
        }
    }
}

impl Default for OpenAiProvider {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL, None)
    }
}

/// Serialize a `ChatRequest` to the OpenAI wire body with prefix-shaping
/// quirks applied. Shared with the `openrouter` adapter, which enriches
/// the resulting JSON before send.
pub(crate) fn build_chat_body(req: &ChatRequest) -> Result<serde_json::Value, ProviderError> {
    let mut body = serde_json::to_value(to_wire(req))
        .map_err(|e| ProviderError::Network(format!("body encode: {e}")))?;
    apply_request_shaping(&mut body, detect_quirks(&req.model))?;
    Ok(body)
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        let body = build_chat_body(&req)?;
        self.transport
            .post_chat_stream(&body, &req.headers, &[], cancel)
            .await
    }

    fn pricing(&self, model: &str) -> Option<ModelPricing> {
        pricing_for(model)
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.transport.list_models().await
    }

    fn capabilities(&self, model: &str) -> ProviderCapabilities {
        // Capabilities mirror the prefix-shaper detection so callers
        // (projector, request builder) get a single source of truth for
        // quirk flags. Vision is OFF by default — the base adapter is the
        // catch-all and shouldn't claim multimodal support unilaterally.
        detect_quirks(model)
    }
}
