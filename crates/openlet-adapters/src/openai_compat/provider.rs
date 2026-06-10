//! `OpenAiCompatProvider` — `ModelProvider` impl streaming OpenRouter via
//! `POST /v1/chat/completions` with `stream: true`.
//!
//! Three-layer split:
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
    ChatRequest, ChatStream, ModelInfo, ModelPricing, ModelProvider, ProviderCapabilities,
};
use openlet_core::error::ProviderError;
use serde::Deserialize;

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
        // or malicious plugin cannot hijack upstream credentials.
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

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let url = format!("{}/models", self.inner.base_url);
        let mut req = self.inner.http.get(&url);
        if let Some(key) = self.inner.api_key.as_ref() {
            // GET /models is free on OpenRouter; the key is sent only so
            // gated catalogs resolve. Mark sensitive so it never lands in
            // a debug log of the request.
            let auth_val = format!("Bearer {}", key.expose_secret());
            let mut auth = HeaderValue::from_str(&auth_val)
                .map_err(|e| ProviderError::Auth(format!("invalid api key header: {e}")))?;
            auth.set_sensitive(true);
            let mut headers = HeaderMap::new();
            headers.insert(AUTHORIZATION, auth);
            req = req.headers(headers);
        }

        let response = req
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(map_http_error(status, response).await);
        }

        let body = response
            .json::<ModelsResponse>()
            .await
            .map_err(|e| ProviderError::Network(format!("models decode: {e}")))?;
        Ok(body.data.into_iter().map(ModelInfo::from).collect())
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
/// credentials by setting `Authorization`.
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
    let body = read_capped_body(resp).await;
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

/// Cap on bytes read from a 4xx/5xx error body before mapping. Prevents
/// a hostile or buggy upstream from OOMing the client by returning a
/// multi-MB error JSON. 64 KiB is generous for human-readable error
/// payloads; the truncate step then trims to 256 chars for display.
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

async fn read_capped_body(resp: reqwest::Response) -> String {
    use futures::StreamExt as _;
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else { break };
        let remaining = MAX_ERROR_BODY_BYTES.saturating_sub(buf.len());
        if remaining == 0 {
            break;
        }
        let take = chunk.len().min(remaining);
        buf.extend_from_slice(&chunk[..take]);
        if buf.len() >= MAX_ERROR_BODY_BYTES {
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Truncate `s` to at most `max` BYTES on a UTF-8 char boundary, then
/// append `…`. Slicing at an arbitrary byte index would panic when the
/// boundary lands inside a multi-byte codepoint (Japanese / Chinese /
/// emoji error JSON).
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// OpenRouter / OpenAI `GET /models` envelope: `{ "data": [ {...}, ... ] }`.
#[derive(Debug, Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

/// One catalog row. Only `id` is required; everything else is best-effort
/// and tolerant of the field being absent or a different shape. OpenRouter
/// nests context length under `context_length`; some gateways use
/// `top_provider.context_length` — we read the flat field and fall back to
/// `None` rather than failing the whole catalog parse.
#[derive(Debug, Deserialize)]
struct ModelEntry {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    context_length: Option<u32>,
}

impl From<ModelEntry> for ModelInfo {
    fn from(e: ModelEntry) -> Self {
        Self {
            id: e.id,
            display_name: e.name,
            context_length: e.context_length,
        }
    }
}
