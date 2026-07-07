//! Shared HTTP transport for the OpenAI-dialect providers.
//!
//! Owns everything that is identical between the base `openai` adapter
//! and the `openrouter` extension: the reqwest client, auth-header
//! assembly, plugin-header merge with reserved-header filtering, 4xx/5xx
//! mapping, capped error-body reads, and `GET /models` parsing. Each
//! provider builds its own request body + provider-specific headers and
//! hands them here for the wire send.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use openlet_core::adapters::model_provider::{ChatStream, ModelInfo};
use openlet_core::error::ProviderError;

use super::stream::spawn_decoder;

/// Reserved header names plugins cannot set via `OnChatHeaders`. Lower-
/// case for case-insensitive comparison. The transport filters these out
/// structurally so a buggy or hostile plugin cannot hijack upstream
/// credentials by setting `Authorization`.
const RESERVED_HEADERS: &[&str] = &[
    "authorization",
    "x-api-key",
    "openai-api-key",
    "anthropic-api-key",
    "openrouter-api-key",
];

/// Cap on bytes read from a 4xx/5xx error body before mapping. Prevents
/// a hostile or buggy upstream from OOMing the client by returning a
/// multi-MB error JSON. 64 KiB is generous for human-readable error
/// payloads; the truncate step then trims to 256 chars for display.
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// Shared HTTP machinery. Cloneable (cheap `Arc` clone of the client).
#[derive(Clone)]
pub struct HttpTransport {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
    api_key: Option<SecretString>,
    http: Client,
    /// Names surfaced in `MissingCredentials` so the operator sees which
    /// provider + env var to set (e.g. `openrouter` / `OPENAI_API_KEY`).
    provider_name: &'static str,
    env_var: &'static str,
}

impl std::fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTransport")
            .field("base_url", &self.inner.base_url)
            .field("provider", &self.inner.provider_name)
            .field("has_key", &self.inner.api_key.is_some())
            .finish()
    }
}

impl HttpTransport {
    #[must_use]
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<SecretString>,
        provider_name: &'static str,
        env_var: &'static str,
    ) -> Self {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Some(Duration::from_secs(90)))
            .build()
            .expect("reqwest client build");
        Self::from_parts(base_url, api_key, http, provider_name, env_var)
    }

    #[must_use]
    pub fn from_parts(
        base_url: impl Into<String>,
        api_key: Option<SecretString>,
        http: Client,
        provider_name: &'static str,
        env_var: &'static str,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                base_url: base_url.into(),
                api_key,
                http,
                provider_name,
                env_var,
            }),
        }
    }

    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.inner.base_url
    }

    /// POST a ready chat-completions body and stream the decoded deltas.
    ///
    /// `builtin_extra` headers are set by the provider (e.g. OpenRouter
    /// attribution) and win over plugin headers. `plugin_headers` come
    /// from `OnChatHeaders` and are merged last with reserved names
    /// filtered structurally.
    pub async fn post_chat_stream(
        &self,
        body: &serde_json::Value,
        plugin_headers: &BTreeMap<String, String>,
        builtin_extra: &[(HeaderName, HeaderValue)],
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        let api_key = self.require_key()?;
        let url = format!("{}/chat/completions", self.inner.base_url);

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(AUTHORIZATION, bearer(api_key)?);
        for (name, val) in builtin_extra {
            headers.insert(name, val.clone());
        }
        merge_plugin_headers(&mut headers, plugin_headers);

        let response = tokio::select! {
            res = self.inner.http.post(&url).headers(headers).json(body).send() => {
                res.map_err(|e| ProviderError::Network(e.to_string()))?
            }
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
        };

        let status = response.status();
        if !status.is_success() {
            return Err(map_http_error(status, response).await);
        }

        let stream = spawn_decoder(response.bytes_stream(), cancel);
        Ok(Box::new(stream))
    }

    /// `GET /models`. Key is sent only so gated catalogs resolve.
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let url = format!("{}/models", self.inner.base_url);
        let mut req = self.inner.http.get(&url);
        if let Some(key) = self.inner.api_key.as_ref() {
            let mut headers = HeaderMap::new();
            headers.insert(AUTHORIZATION, bearer(key)?);
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

    fn require_key(&self) -> Result<&SecretString, ProviderError> {
        self.inner
            .api_key
            .as_ref()
            .ok_or(ProviderError::MissingCredentials {
                provider: self.inner.provider_name,
                env_var: self.inner.env_var,
            })
    }
}

/// Build a sensitive `Authorization: Bearer …` header value.
fn bearer(key: &SecretString) -> Result<HeaderValue, ProviderError> {
    let mut auth = HeaderValue::from_str(&format!("Bearer {}", key.expose_secret()))
        .map_err(|e| ProviderError::Auth(format!("invalid api key header: {e}")))?;
    auth.set_sensitive(true);
    Ok(auth)
}

/// Merge `OnChatHeaders` plugin headers, dropping reserved (auth-bearing)
/// names and anything already present so built-in headers win.
fn merge_plugin_headers(headers: &mut HeaderMap, plugin_headers: &BTreeMap<String, String>) {
    for (k, v) in plugin_headers {
        if RESERVED_HEADERS.contains(&k.to_ascii_lowercase().as_str()) {
            tracing::warn!(header = %k, "plugin attempted to set reserved header; ignoring");
            continue;
        }
        let Ok(name) = HeaderName::from_bytes(k.as_bytes()) else {
            tracing::warn!(header = %k, "plugin header name invalid; ignoring");
            continue;
        };
        let Ok(val) = HeaderValue::from_str(v) else {
            tracing::warn!(header = %k, "plugin header value invalid; ignoring");
            continue;
        };
        headers.entry(&name).or_insert(val);
    }
}

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

/// `GET /models` envelope: `{ "data": [ {...}, ... ] }`.
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
