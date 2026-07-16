//! `web_fetch` tool — fetch a URL and return its content as markdown or raw
//! HTML, backed by an injected [`WebFetcher`].
//!
//! The `WebFetcher` seam is defined INLINE here (mirroring `ShellExecutor`
//! in `bash.rs` and `PythonExecutor` in `python.rs`), NOT in
//! `adapters/mod.rs` — the six core adapter traits are unchanged; this is a
//! tool-local injection seam. The concrete network impl (`ReqwestWebFetcher`)
//! lives in `openlet-adapters` and is `Option`-injected like `python`, so a
//! network-free integrator that wires no fetcher simply has no `web_fetch`
//! tool registered.
//!
//! This is the runtime's ONLY outbound-network capability. The URL is
//! model-controlled and IS the exfil channel, so egress is gated two ways:
//! a `web_fetch:**` → Ask permission seed (human-in-the-loop, never silent)
//! AND the IP-pinned SSRF guard below. [`is_blocked_ip`] is a pure,
//! unit-testable classifier; the adapter resolves the host once, classifies
//! the resulting IP, and pins the connection to that vetted address so a
//! DNS-rebind between check and connect cannot redirect to an internal host.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::Tool;
use crate::types::permission::PermissionRequest;

/// Output content conversion. `Markdown` (default) runs HTML through the
/// converter; `Html` returns the raw body verbatim (zero conversion deps).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FetchFormat {
    /// HTML converted to markdown (textual passthrough for non-HTML MIME).
    #[default]
    Markdown,
    /// Raw response body, no conversion.
    Html,
}

/// A single fetch request handed to the [`WebFetcher`].
#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub url: String,
    pub format: FetchFormat,
}

/// A successfully fetched + converted page.
#[derive(Debug, Clone)]
pub struct FetchedPage {
    /// Final URL after any redirects.
    pub url_final: String,
    /// Converted (markdown) or raw (html/text) content, already truncated
    /// on a UTF-8 char boundary by the fetcher.
    pub content: String,
    /// The response `content-type` header (sans parameters).
    pub content_type: String,
    /// Whether `content` was truncated to the output cap.
    pub truncated: bool,
    /// Number of DECODED body bytes read before truncation.
    pub bytes: usize,
}

/// Errors a [`WebFetcher`] can return. Mapped to [`ToolError`] with an
/// actionable message at the tool boundary.
#[derive(Debug, Clone, thiserror::Error)]
pub enum FetchError {
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    /// The resolved host / redirect target is a private, loopback,
    /// link-local, metadata, or otherwise non-public address.
    #[error("blocked host (SSRF guard): {0}")]
    BlockedHost(String),
    #[error("content type not textual: {0}")]
    UnsupportedContentType(String),
    #[error("response exceeded the byte cap")]
    TooLarge,
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("http status {0}")]
    Http(u16),
    #[error("request timed out")]
    Timeout,
    #[error("network error: {0}")]
    Network(String),
    #[error("content decode error: {0}")]
    Decode(String),
}

impl From<FetchError> for ToolError {
    fn from(e: FetchError) -> Self {
        match e {
            FetchError::InvalidUrl(m) => ToolError::InvalidInput(format!("invalid url: {m}")),
            FetchError::BlockedHost(m) => ToolError::PermissionDenied(format!(
                "web_fetch refused to connect to a non-public address ({m}). \
                 The SSRF guard blocks internal/loopback/link-local hosts."
            )),
            FetchError::UnsupportedContentType(ct) => ToolError::InvalidInput(format!(
                "web_fetch can only return textual content; got content-type '{ct}'"
            )),
            FetchError::TooLarge => {
                ToolError::InvalidInput("web_fetch response exceeded the byte cap".into())
            }
            FetchError::TooManyRedirects => {
                ToolError::InvalidInput("web_fetch hit the redirect limit".into())
            }
            FetchError::Http(code) => ToolError::Io(format!("web_fetch got HTTP status {code}")),
            FetchError::Timeout => ToolError::Timeout,
            FetchError::Network(m) => ToolError::Io(format!("web_fetch network error: {m}")),
            FetchError::Decode(m) => {
                ToolError::Io(format!("web_fetch could not decode the response: {m}"))
            }
        }
    }
}

/// Object-safe seam the runtime injects into [`WebFetchTool`]. Implemented
/// in `openlet-adapters` as `ReqwestWebFetcher`. Defined inline here (like
/// [`super::bash::ShellExecutor`]) rather than as a core adapter trait.
#[async_trait]
pub trait WebFetcher: Send + Sync + 'static {
    async fn fetch(&self, req: FetchRequest) -> Result<FetchedPage, FetchError>;
}

/// Classify an IP address as non-public (must be refused by the SSRF guard).
///
/// Pure and unit-testable — the adapter resolves the host, then calls this
/// on the resulting `IpAddr` BEFORE connecting, and pins the socket to the
/// vetted address. IPv4-mapped IPv6 (`::ffff:a.b.c.d`) and the NAT64
/// well-known prefix (`64:ff9b::/96`) are normalized to their embedded IPv4
/// before classification so a mapped internal address cannot slip through.
#[must_use]
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_blocked_v4(mapped);
            }
            if let Some(embedded) = nat64_embedded_v4(v6) {
                return is_blocked_v4(embedded);
            }
            if let Some(embedded) = six_to_four_embedded_v4(v6) {
                return is_blocked_v4(embedded);
            }
            is_blocked_v6(v6)
        }
    }
}

fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()            // 127.0.0.0/8
        || ip.is_private()      // 10/8, 172.16/12, 192.168/16
        || ip.is_link_local()   // 169.254.0.0/16 (incl. 169.254.169.254 metadata)
        || ip.is_broadcast()    // 255.255.255.255
        || ip.is_multicast()    // 224.0.0.0/4
        || ip.is_unspecified()  // 0.0.0.0
        || ip.is_documentation()// 192.0.2/24, 198.51.100/24, 203.0.113/24
        || o[0] == 0            // 0.0.0.0/8 "this network"
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // 100.64.0.0/10 CGNAT
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19)) // 198.18.0.0/15 benchmark
        || o[0] >= 240 // 240.0.0.0/4 reserved
}

fn is_blocked_v6(ip: Ipv6Addr) -> bool {
    let seg = ip.segments();
    ip.is_loopback()            // ::1
        || ip.is_unspecified()  // ::
        || ip.is_multicast()    // ff00::/8
        || (seg[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        || (seg[0] & 0xfe00) == 0xfc00 // fc00::/7 unique-local (ULA)
        || (seg[0] == 0x0100 && seg[1] == 0 && seg[2] == 0 && seg[3] == 0) // 100::/64 discard-only
        || (seg[0] == 0x2001 && seg[1] == 0) // 2001::/32 Teredo transition
        || (seg[0] == 0x2001 && seg[1] == 0x0002) // 2001:2::/48 benchmark
        || (seg[0] == 0x2001 && seg[1] == 0x0db8) // 2001:db8::/32 documentation
}

/// Extract the embedded IPv4 from a NAT64 well-known-prefix address
/// (`64:ff9b::/96`), if `ip` is in that prefix.
fn nat64_embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let seg = ip.segments();
    let is_nat64 = seg[0] == 0x0064
        && seg[1] == 0xff9b
        && seg[2] == 0
        && seg[3] == 0
        && seg[4] == 0
        && seg[5] == 0;
    if is_nat64 {
        Some(Ipv4Addr::new(
            (seg[6] >> 8) as u8,
            (seg[6] & 0xff) as u8,
            (seg[7] >> 8) as u8,
            (seg[7] & 0xff) as u8,
        ))
    } else {
        None
    }
}

/// Extract the embedded IPv4 from a 6to4 address (`2002::/16`). A private
/// embedded IPv4 must remain blocked even though the outer address looks like
/// globally routed IPv6. Teredo (`2001::/32`) is blocked wholesale above
/// because its obfuscated endpoint mapping is not safe to classify here.
fn six_to_four_embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let seg = ip.segments();
    (seg[0] == 0x2002).then(|| {
        Ipv4Addr::new(
            (seg[1] >> 8) as u8,
            (seg[1] & 0xff) as u8,
            (seg[2] >> 8) as u8,
            (seg[2] & 0xff) as u8,
        )
    })
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WebFetchInput {
    /// Absolute `http`/`https` URL to fetch.
    pub url: String,
    /// Output format: `markdown` (default, HTML converted to markdown) or
    /// `html` (raw body).
    #[serde(default)]
    pub format: Option<FetchFormat>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WebFetchOutput {
    /// Final URL after redirects.
    pub url: String,
    /// Page content (markdown or raw html), UTF-8-safe truncated.
    pub content: String,
    /// Response content-type.
    pub content_type: String,
    /// Whether `content` was truncated to the output cap.
    pub truncated: bool,
}

#[derive(Default)]
pub struct WebFetchTool {
    fetcher: Option<Arc<dyn WebFetcher>>,
}

impl WebFetchTool {
    #[must_use]
    pub fn with_fetcher(fetcher: Arc<dyn WebFetcher>) -> Self {
        Self {
            fetcher: Some(fetcher),
        }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    type Input = WebFetchInput;
    type Output = WebFetchOutput;

    fn name(&self) -> &'static str {
        "web_fetch"
    }
    fn description(&self) -> &'static str {
        "Fetch a URL and return its content. `format`: markdown (default; HTML \
         converted to markdown) or html (raw body). Only http/https; internal \
         and loopback addresses are refused. Output is size-capped."
    }
    fn parallel_safe(&self) -> bool {
        // Read-only, no filesystem mutation.
        true
    }

    fn permission(&self, input: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple(format!("web_fetch:{}", input.url))
    }

    async fn run(&self, _ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        let fetcher = self
            .fetcher
            .as_ref()
            .ok_or_else(|| ToolError::Io("web fetcher not configured".into()))?;
        let page = fetcher
            .fetch(FetchRequest {
                url: input.url,
                format: input.format.unwrap_or_default(),
            })
            .await?;
        Ok(WebFetchOutput {
            url: page.url_final,
            content: page.content,
            content_type: page.content_type,
            truncated: page.truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(Ipv4Addr::from_str(s).unwrap())
    }
    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(Ipv6Addr::from_str(s).unwrap())
    }

    #[test]
    fn blocks_loopback_and_private_v4() {
        assert!(is_blocked_ip(v4("127.0.0.1")));
        assert!(is_blocked_ip(v4("10.0.0.1")));
        assert!(is_blocked_ip(v4("172.16.5.4")));
        assert!(is_blocked_ip(v4("192.168.1.1")));
    }

    #[test]
    fn blocks_link_local_and_metadata_v4() {
        assert!(is_blocked_ip(v4("169.254.0.1")));
        // AWS/GCP metadata endpoint.
        assert!(is_blocked_ip(v4("169.254.169.254")));
    }

    #[test]
    fn blocks_this_network_and_cgnat_and_reserved_v4() {
        assert!(is_blocked_ip(v4("0.0.0.0")));
        assert!(is_blocked_ip(v4("0.1.2.3"))); // 0.0.0.0/8
        assert!(is_blocked_ip(v4("100.64.0.1"))); // CGNAT
        assert!(is_blocked_ip(v4("100.127.255.255"))); // CGNAT edge
        assert!(is_blocked_ip(v4("240.0.0.1"))); // reserved
        assert!(is_blocked_ip(v4("255.255.255.255"))); // broadcast
    }

    #[test]
    fn allows_public_v4() {
        assert!(!is_blocked_ip(v4("8.8.8.8")));
        assert!(!is_blocked_ip(v4("1.1.1.1")));
        assert!(!is_blocked_ip(v4("93.184.216.34"))); // example.com
        assert!(!is_blocked_ip(v4("100.63.255.255"))); // just below CGNAT
        assert!(!is_blocked_ip(v4("100.128.0.0"))); // just above CGNAT
    }

    #[test]
    fn blocks_loopback_ula_linklocal_v6() {
        assert!(is_blocked_ip(v6("::1"))); // loopback
        assert!(is_blocked_ip(v6("::"))); // unspecified
        assert!(is_blocked_ip(v6("fe80::1"))); // link-local
        assert!(is_blocked_ip(v6("fc00::1"))); // ULA
        assert!(is_blocked_ip(v6("fd12:3456::1"))); // ULA
        assert!(is_blocked_ip(v6("ff02::1"))); // multicast
    }

    #[test]
    fn blocks_ipv4_mapped_v6() {
        // IPv4-mapped IPv6 must be normalized before classifying.
        assert!(is_blocked_ip(v6("::ffff:127.0.0.1")));
        assert!(is_blocked_ip(v6("::ffff:169.254.169.254")));
        assert!(is_blocked_ip(v6("::ffff:10.0.0.1")));
        // A mapped PUBLIC address is still allowed.
        assert!(!is_blocked_ip(v6("::ffff:8.8.8.8")));
    }

    #[test]
    fn blocks_nat64_embedded_internal() {
        // 64:ff9b::/96 embedding 169.254.169.254 → blocked.
        assert!(is_blocked_ip(v6("64:ff9b::a9fe:a9fe")));
        // 64:ff9b:: embedding 8.8.8.8 → allowed.
        assert!(!is_blocked_ip(v6("64:ff9b::808:808")));
    }

    #[test]
    fn blocks_transition_and_reserved_ranges() {
        assert!(is_blocked_ip(v4("198.18.0.1"))); // benchmark network
        assert!(is_blocked_ip(v6("100::1"))); // discard-only
        assert!(is_blocked_ip(v6("2001::1"))); // Teredo transition
        assert!(is_blocked_ip(v6("2001:2::1"))); // benchmark
        assert!(is_blocked_ip(v6("2001:db8::1"))); // documentation
        // 6to4 embedding 10.0.0.1 must normalize and block.
        assert!(is_blocked_ip(v6("2002:a00:1::1")));
        // 6to4 embedding public 8.8.8.8 remains allowed.
        assert!(!is_blocked_ip(v6("2002:808:808::1")));
    }

    #[test]
    fn allows_public_v6() {
        assert!(!is_blocked_ip(v6("2606:4700:4700::1111"))); // cloudflare
        assert!(!is_blocked_ip(v6("2001:4860:4860::8888"))); // google
    }

    #[test]
    fn format_defaults_to_markdown() {
        assert_eq!(FetchFormat::default(), FetchFormat::Markdown);
        let input: WebFetchInput =
            serde_json::from_value(serde_json::json!({ "url": "https://x.test" })).unwrap();
        assert_eq!(input.format.unwrap_or_default(), FetchFormat::Markdown);
    }

    #[test]
    fn format_parses_html() {
        let input: WebFetchInput = serde_json::from_value(
            serde_json::json!({ "url": "https://x.test", "format": "html" }),
        )
        .unwrap();
        assert_eq!(input.format, Some(FetchFormat::Html));
    }
}
