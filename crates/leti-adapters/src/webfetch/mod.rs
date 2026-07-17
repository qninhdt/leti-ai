//! `ReqwestWebFetcher` — the concrete [`WebFetcher`] impl backing the
//! `web_fetch` tool.
//!
//! Security model (this is the runtime's ONLY outbound-network capability):
//! - **Scheme allowlist**: only `http` / `https`; `user:pass@` credentials in
//!   the URL are refused.
//! - **IP-pinning against DNS-rebind**: the host is resolved ONCE (blocking
//!   std resolver), every resolved address is classified with the pure
//!   [`is_blocked_ip`] guard, and the connection is pinned to the vetted
//!   public IP via reqwest's `.resolve()` DNS override (Host + SNI preserved
//!   for TLS). reqwest cannot re-resolve to a different address between the
//!   check and the connect.
//! - **Per-hop re-check**: redirects are handled manually (`redirect::none`);
//!   scheme, credential, and SSRF checks re-run on EVERY hop, capped at
//!   [`MAX_REDIRECTS`].
//! - **OOM defense**: the raw body is streamed and aborted the instant it
//!   crosses [`BYTE_CAP`] — an oversized (or lying `Content-Length`) body can
//!   never be buffered whole. Compression is NOT negotiated (the client does
//!   not advertise `Accept-Encoding`), so there is no decompression-bomb
//!   inflation path.
//! - **UTF-8-safe output**: the decoded body is lossy-decoded then truncated
//!   on a char boundary to [`OUTPUT_CHAR_CAP`].

mod convert;

use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt as _;
use reqwest::Client;
use reqwest::header::{ACCEPT_ENCODING, HeaderValue};
use reqwest::redirect::Policy;
use url::Url;

use leti_core::tools::builtins::web_fetch::{
    FetchError, FetchRequest, FetchedPage, WebFetcher, is_blocked_ip,
};

use convert::convert_body;

/// Max redirect hops before giving up.
const MAX_REDIRECTS: usize = 10;
/// Hard cap on RAW downloaded bytes. Crossing this aborts the stream with
/// [`FetchError::TooLarge`] — the OOM guard.
const BYTE_CAP: usize = 5 * 1024 * 1024;
/// Output content is truncated to this many CHARS (UTF-8-boundary safe).
const OUTPUT_CHAR_CAP: usize = 100_000;
/// Default overall request timeout.
const DEFAULT_TIMEOUT_SECS: u64 = 30;
/// Honest User-Agent so operators of fetched hosts can identify the client.
const USER_AGENT: &str = concat!("leti-web-fetch/", env!("CARGO_PKG_VERSION"));

/// The concrete network fetcher. Cheap to clone (holds nothing but config).
#[derive(Debug, Clone)]
pub struct ReqwestWebFetcher {
    timeout: Duration,
    /// Crate-internal test seam ONLY. When `true`, the SSRF guard permits
    /// loopback/private addresses so in-crate `wiremock` tests (which bind
    /// `127.0.0.1`) can exercise the fetch path. NEVER settable from outside
    /// this crate — `new()` always leaves it `false`, so the model-facing
    /// tool always runs the full guard.
    allow_private: bool,
}

impl Default for ReqwestWebFetcher {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            allow_private: false,
        }
    }
}

impl ReqwestWebFetcher {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Test-only constructor that relaxes the SSRF guard to allow loopback,
    /// so `wiremock` (which binds `127.0.0.1`) can be exercised. `pub(crate)`
    /// — unreachable from the tool / server / model.
    #[cfg(test)]
    pub(crate) fn for_test_allowing_loopback(timeout: Duration) -> Self {
        Self {
            timeout,
            allow_private: true,
        }
    }
}

/// Validate scheme + credentials on a URL. Returns the lowercased host.
fn validate_url(url: &Url) -> Result<String, FetchError> {
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(FetchError::InvalidUrl(format!(
                "unsupported scheme '{other}' (only http/https)"
            )));
        }
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(FetchError::InvalidUrl(
            "credentials in URL (user:pass@) are not allowed".into(),
        ));
    }
    url.host_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| FetchError::InvalidUrl("URL has no host".into()))
}

/// Resolve `host:port` to socket addresses and return the FIRST vetted
/// (non-blocked) address. All resolved addresses are classified; if none are
/// public the request is refused. Runs the blocking std resolver on the
/// blocking pool so the async runtime is never stalled.
async fn resolve_vetted_addr(
    host: &str,
    port: u16,
    allow_private: bool,
) -> Result<SocketAddr, FetchError> {
    let host_owned = host.to_string();
    let addrs = tokio::task::spawn_blocking(move || {
        (host_owned.as_str(), port)
            .to_socket_addrs()
            .map(|it| it.collect::<Vec<_>>())
    })
    .await
    .map_err(|e| FetchError::Network(format!("resolver join: {e}")))?
    .map_err(|e| FetchError::Network(format!("dns resolution failed: {e}")))?;

    if addrs.is_empty() {
        return Err(FetchError::Network(format!("no addresses for host {host}")));
    }
    // Pin to the first PUBLIC address. Any blocked address in the set means
    // we simply don't select it; if every address is blocked we refuse.
    // `allow_private` (test-only) short-circuits the guard for loopback mocks.
    addrs
        .into_iter()
        .find(|sa| allow_private || !is_blocked_ip(sa.ip()))
        .ok_or_else(|| FetchError::BlockedHost(host.to_string()))
}

/// Build a one-shot reqwest client that pins `host` to the vetted socket
/// address. Redirects are disabled — we drive the hop loop ourselves so the
/// SSRF checks re-run per hop. Compression is not negotiated. System proxies
/// are explicitly disabled: a proxy would resolve the hostname itself and
/// therefore bypass this client's vetted-IP DNS pinning.
fn build_pinned_client(
    host: &str,
    vetted: SocketAddr,
    timeout: Duration,
) -> Result<Client, FetchError> {
    // Port 0 lets reqwest use the URL/scheme's conventional port while still
    // pinning the IP.
    let pin = SocketAddr::new(vetted.ip(), 0);
    Client::builder()
        .redirect(Policy::none())
        .no_proxy()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT)
        .resolve(host, pin)
        .build()
        .map_err(|e| FetchError::Network(format!("client build: {e}")))
}

/// Read the body stream, aborting the instant the accumulated RAW bytes
/// cross [`BYTE_CAP`]. Never buffers a whole oversized body.
async fn read_capped(resp: reqwest::Response) -> Result<Vec<u8>, FetchError> {
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| classify_reqwest_err(&e))?;
        if buf.len() + chunk.len() > BYTE_CAP {
            return Err(FetchError::TooLarge);
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

/// Truncate `content` to at most [`OUTPUT_CHAR_CAP`] chars on a char
/// boundary. Returns `(truncated_content, was_truncated)`.
fn truncate_output(content: String) -> (String, bool) {
    if content.chars().count() <= OUTPUT_CHAR_CAP {
        return (content, false);
    }
    let mut out = String::with_capacity(OUTPUT_CHAR_CAP);
    for ch in content.chars().take(OUTPUT_CHAR_CAP) {
        out.push(ch);
    }
    (out, true)
}

/// Map a reqwest error into the closest [`FetchError`].
fn classify_reqwest_err(e: &reqwest::Error) -> FetchError {
    if e.is_timeout() {
        FetchError::Timeout
    } else if e.is_connect() {
        // A pinned connect that fails is most useful surfaced as a network
        // error; the SSRF guard already ran before we ever connected.
        FetchError::Network(format!("connect failed: {e}"))
    } else {
        FetchError::Network(e.to_string())
    }
}

/// The `content-type` header value, sans parameters, or a sensible default.
fn content_type_of(resp: &reqwest::Response) -> String {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

#[async_trait]
impl WebFetcher for ReqwestWebFetcher {
    async fn fetch(&self, req: FetchRequest) -> Result<FetchedPage, FetchError> {
        let mut current = Url::parse(&req.url)
            .map_err(|e| FetchError::InvalidUrl(format!("{}: {e}", req.url)))?;

        for _hop in 0..=MAX_REDIRECTS {
            // Per-hop: scheme + credential + SSRF (IP-pinned) checks.
            let host = validate_url(&current)?;
            let port = current.port_or_known_default().unwrap_or(0);
            let vetted = resolve_vetted_addr(&host, port, self.allow_private).await?;
            let client = build_pinned_client(&host, vetted, self.timeout)?;

            let resp = client
                .get(current.clone())
                // Compression is deliberately disabled. The fetcher caps the
                // raw stream, so a small archive can never inflate into an
                // unbounded decoded body. Servers that ignore `identity` are
                // still subject to the raw byte cap.
                .header(ACCEPT_ENCODING, HeaderValue::from_static("identity"))
                .send()
                .await
                .map_err(|e| classify_reqwest_err(&e))?;

            let status = resp.status();
            if status.is_redirection() {
                let location = resp
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| {
                        FetchError::Network(format!("redirect {status} without Location"))
                    })?;
                // Resolve relative redirects against the current URL, then
                // loop so the next iteration re-runs every guard.
                current = current
                    .join(location)
                    .map_err(|e| FetchError::InvalidUrl(format!("bad redirect target: {e}")))?;
                continue;
            }

            if !status.is_success() {
                return Err(FetchError::Http(status.as_u16()));
            }

            let content_type = content_type_of(&resp);
            let url_final = resp.url().to_string();
            let raw = read_capped(resp).await?;
            let bytes = raw.len();
            // Lossy UTF-8 decode: a non-UTF-8 textual body still yields
            // readable content rather than an error.
            let decoded = String::from_utf8_lossy(&raw).into_owned();
            let converted = convert_body(&decoded, &content_type, req.format)?;
            let (content, truncated) = truncate_output(converted);

            return Ok(FetchedPage {
                url_final,
                content,
                content_type,
                truncated,
                bytes,
            });
        }

        Err(FetchError::TooManyRedirects)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http_scheme() {
        let url = Url::parse("ftp://example.com/x").unwrap();
        assert!(matches!(validate_url(&url), Err(FetchError::InvalidUrl(_))));
    }

    #[test]
    fn rejects_file_scheme() {
        let url = Url::parse("file:///etc/passwd").unwrap();
        assert!(matches!(validate_url(&url), Err(FetchError::InvalidUrl(_))));
    }

    #[test]
    fn rejects_url_credentials() {
        let url = Url::parse("http://user:pass@example.com/").unwrap();
        assert!(matches!(validate_url(&url), Err(FetchError::InvalidUrl(_))));
    }

    #[test]
    fn accepts_plain_http_https() {
        assert_eq!(
            validate_url(&Url::parse("http://Example.COM/path").unwrap()).unwrap(),
            "example.com"
        );
        assert_eq!(
            validate_url(&Url::parse("https://example.com/").unwrap()).unwrap(),
            "example.com"
        );
    }

    #[test]
    fn truncate_flags_when_over_cap() {
        let big: String = "a".repeat(OUTPUT_CHAR_CAP + 10);
        let (out, truncated) = truncate_output(big);
        assert!(truncated);
        assert_eq!(out.chars().count(), OUTPUT_CHAR_CAP);
    }

    #[test]
    fn truncate_preserves_char_boundary() {
        // Multi-byte chars: truncation must not split a codepoint.
        let s: String = "é".repeat(OUTPUT_CHAR_CAP + 5);
        let (out, truncated) = truncate_output(s);
        assert!(truncated);
        assert_eq!(out.chars().count(), OUTPUT_CHAR_CAP);
        // Round-trips as valid UTF-8 (no panic / no partial codepoint).
        assert!(out.chars().all(|c| c == 'é'));
    }

    #[test]
    fn short_content_not_truncated() {
        let (out, truncated) = truncate_output("hello".into());
        assert!(!truncated);
        assert_eq!(out, "hello");
    }

    // ---- wiremock integration (loopback allowed via the test seam) ----

    use leti_core::tools::builtins::web_fetch::FetchFormat;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_fetcher() -> ReqwestWebFetcher {
        ReqwestWebFetcher::for_test_allowing_loopback(Duration::from_secs(10))
    }

    #[tokio::test]
    async fn fetches_html_and_converts_to_markdown() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/page"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw("<h1>Title</h1><p>Body</p>", "text/html; charset=utf-8"),
            )
            .mount(&server)
            .await;

        let page = test_fetcher()
            .fetch(FetchRequest {
                url: format!("{}/page", server.uri()),
                format: FetchFormat::Markdown,
            })
            .await
            .unwrap();
        // htmd renders the heading + paragraph; assert both texts survive the
        // conversion (exact heading syntax is htmd's concern, not ours).
        assert!(page.content.contains("Title"));
        assert!(page.content.contains("Body"));
        assert_eq!(page.content_type, "text/html");
        assert!(!page.truncated);
    }

    #[tokio::test]
    async fn html_format_returns_raw_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/raw"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string("<h1>Raw</h1>"),
            )
            .mount(&server)
            .await;

        let page = test_fetcher()
            .fetch(FetchRequest {
                url: format!("{}/raw", server.uri()),
                format: FetchFormat::Html,
            })
            .await
            .unwrap();
        assert_eq!(page.content, "<h1>Raw</h1>");
    }

    #[tokio::test]
    async fn follows_redirect_within_cap() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/from"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/to"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/to"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_string("landed"),
            )
            .mount(&server)
            .await;

        let page = test_fetcher()
            .fetch(FetchRequest {
                url: format!("{}/from", server.uri()),
                format: FetchFormat::Markdown,
            })
            .await
            .unwrap();
        assert_eq!(page.content, "landed");
        assert!(page.url_final.ends_with("/to"));
    }

    #[tokio::test]
    async fn redirect_loop_exceeds_cap() {
        let server = MockServer::start().await;
        // Every hop 302s back to the same self-referential path.
        Mock::given(method("GET"))
            .and(path("/loop"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/loop"))
            .mount(&server)
            .await;

        let err = test_fetcher()
            .fetch(FetchRequest {
                url: format!("{}/loop", server.uri()),
                format: FetchFormat::Markdown,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::TooManyRedirects));
    }

    #[tokio::test]
    async fn oversized_body_aborts_per_chunk() {
        let server = MockServer::start().await;
        // A body larger than the cap. reqwest streams it; read_capped must
        // abort before buffering the whole thing.
        let huge = "a".repeat(BYTE_CAP + 1024);
        Mock::given(method("GET"))
            .and(path("/big"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_string(huge),
            )
            .mount(&server)
            .await;

        let err = test_fetcher()
            .fetch(FetchRequest {
                url: format!("{}/big", server.uri()),
                format: FetchFormat::Markdown,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::TooLarge));
    }

    #[tokio::test]
    async fn http_error_status_surfaces() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/nope"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let err = test_fetcher()
            .fetch(FetchRequest {
                url: format!("{}/nope", server.uri()),
                format: FetchFormat::Markdown,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Http(404)));
    }

    #[tokio::test]
    async fn binary_content_type_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/img"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(vec![0x89, 0x50, 0x4e, 0x47]),
            )
            .mount(&server)
            .await;

        let err = test_fetcher()
            .fetch(FetchRequest {
                url: format!("{}/img", server.uri()),
                format: FetchFormat::Markdown,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::UnsupportedContentType(_)));
    }

    #[tokio::test]
    async fn dns_rebind_blocked_when_guard_active() {
        // A production fetcher (guard ON) must refuse a loopback URL — this is
        // the DNS-rebind / SSRF defense. The mock server binds 127.0.0.1.
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/x"))
            .respond_with(ResponseTemplate::new(200).set_body_string("secret"))
            .mount(&server)
            .await;

        let err = ReqwestWebFetcher::new()
            .fetch(FetchRequest {
                url: format!("{}/x", server.uri()),
                format: FetchFormat::Markdown,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::BlockedHost(_)));
    }
}
