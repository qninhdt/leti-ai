//! `OpenAiCompatProvider` — `ModelProvider` impl streaming OpenRouter via
//! `POST /v1/chat/completions` with `stream: true`.
//!
//! Three-layer split (per phase-03 §Architecture):
//!  1. This file — HTTP send + cancellation + 4xx/5xx mapping. No domain.
//!  2. `wire.rs` — `ChatRequest` ↔ OpenAI JSON request shape.
//!  3. `sse.rs` + `chunk_decoder.rs` — frame extraction + `ChatDelta` decode.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Client, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use tokio_util::sync::CancellationToken;

use openlet_core::adapters::model_provider::{
    ChatRequest, ChatStream, ModelPricing, ModelProvider, ProviderCapabilities,
};
use openlet_core::error::ProviderError;

use super::prefix_shaping::{apply_request_shaping, detect_quirks};
use super::pricing::pricing_for;
use super::stream::spawn_decoder;
use super::wire::to_wire;

/// Default OpenRouter base URL. Override via `OpenAiCompatProvider::new`
/// for self-hosted gateways or testing against `wiremock`.
pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

#[derive(Clone)]
pub struct OpenAiCompatProvider {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
    api_key: Option<SecretString>,
    http: Client,
}

impl std::fmt::Debug for OpenAiCompatProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiCompatProvider")
            .field("base_url", &self.inner.base_url)
            .field("has_key", &self.inner.api_key.is_some())
            .finish()
    }
}

impl OpenAiCompatProvider {
    /// Build with explicit configuration. Prefer `from_env` outside tests.
    #[must_use]
    pub fn new(base_url: impl Into<String>, api_key: Option<SecretString>) -> Self {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Some(Duration::from_secs(90)))
            .build()
            .expect("reqwest client build");
        Self {
            inner: Arc::new(Inner {
                base_url: base_url.into(),
                api_key,
                http,
            }),
        }
    }

    /// Convenience constructor used by `openlet-server::main` once env-based
    /// `Config` has been parsed.
    #[must_use]
    pub fn from_parts(base_url: String, api_key: Option<SecretString>, http: Client) -> Self {
        Self {
            inner: Arc::new(Inner {
                base_url,
                api_key,
                http,
            }),
        }
    }
}

impl Default for OpenAiCompatProvider {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL, None)
    }
}

#[async_trait]
impl ModelProvider for OpenAiCompatProvider {
    async fn chat_stream(
        &self,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        let api_key =
            self.inner
                .api_key
                .as_ref()
                .ok_or_else(|| ProviderError::MissingCredentials {
                    provider: "openrouter",
                    env_var: "OPENROUTER_API_KEY",
                })?;

        let mut body = serde_json::to_value(to_wire(&req))
            .map_err(|e| ProviderError::Network(format!("body encode: {e}")))?;
        let caps = detect_quirks(&req.model);
        apply_request_shaping(&mut body, caps)?;
        let url = format!("{}/chat/completions", self.inner.base_url);

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let auth_val = format!("Bearer {}", api_key.expose_secret());
        let mut auth = HeaderValue::from_str(&auth_val)
            .map_err(|e| ProviderError::Auth(format!("invalid api key header: {e}")))?;
        auth.set_sensitive(true);
        headers.insert(AUTHORIZATION, auth);

        // Merge plugin-injected headers from `OnChatHeaders`. Reserved
        // headers (auth-bearing) are filtered structurally so a buggy
        // or malicious plugin cannot hijack upstream credentials. Closes
        // SA-F3 (plugin Authorization hijack via doc-only protection).
        for (k, v) in &req.headers {
            let lk = k.to_ascii_lowercase();
            if RESERVED_HEADERS.contains(&lk.as_str()) {
                tracing::warn!(
                    header = %k,
                    "plugin attempted to set reserved header; ignoring"
                );
                continue;
            }
            let Ok(name) = reqwest::header::HeaderName::from_bytes(k.as_bytes()) else {
                tracing::warn!(header = %k, "plugin header name invalid; ignoring");
                continue;
            };
            let Ok(val) = HeaderValue::from_str(v) else {
                tracing::warn!(header = %k, "plugin header value invalid; ignoring");
                continue;
            };
            // Insert only if not already present (built-in wins).
            headers.entry(&name).or_insert(val);
        }

        let response = tokio::select! {
            res = self.inner.http.post(&url).headers(headers).json(&body).send() => {
                res.map_err(|e| ProviderError::Network(e.to_string()))?
            }
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
        };

        let status = response.status();
        if !status.is_success() {
            return Err(map_http_error(status, response).await);
        }

        let bytes_stream = response.bytes_stream();
        let stream = spawn_decoder(bytes_stream, cancel);
        Ok(Box::new(stream))
    }

    fn pricing(&self, model: &str) -> Option<ModelPricing> {
        pricing_for(model)
    }

    fn capabilities(&self, model: &str) -> ProviderCapabilities {
        // Capabilities mirror the prefix-shaper detection so callers
        // (router, projector, request builder) get a single source of
        // truth for quirk flags. Vision is OFF by default — the
        // OpenAI-compat adapter is the catch-all and shouldn't claim
        // multimodal support unilaterally.
        detect_quirks(model)
    }
}

/// Reserved header names plugins cannot set via `OnChatHeaders`. Lower-
/// case for case-insensitive comparison. The adapter filters these out
/// structurally so a buggy or hostile plugin cannot hijack upstream
/// credentials by setting `Authorization`. Closes SA-F3.
const RESERVED_HEADERS: &[&str] = &[
    "authorization",
    "x-api-key",
    "openai-api-key",
    "anthropic-api-key",
    "openrouter-api-key",
];

async fn map_http_error(status: StatusCode, resp: reqwest::Response) -> ProviderError {
    let retry_after_ms = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
        .map(|s| s.saturating_mul(1_000))
        .unwrap_or(1_000);
    let body = resp.text().await.unwrap_or_default();
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
            ProviderError::Auth(truncate(&body, 256))
        }
        StatusCode::TOO_MANY_REQUESTS => ProviderError::RateLimit { retry_after_ms },
        s if s.is_server_error() => {
            ProviderError::Network(format!("{s}: {}", truncate(&body, 256)))
        }
        s => ProviderError::Network(format!("{s}: {}", truncate(&body, 256))),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
