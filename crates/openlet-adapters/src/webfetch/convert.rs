//! Content conversion for `web_fetch`: HTML→markdown (via `htmd`) plus the
//! textual-MIME passthrough gate. Kept separate so the conversion + MIME
//! rules are unit-testable without any network.

use openlet_core::tools::builtins::web_fetch::{FetchError, FetchFormat};

/// True when `content_type` names a textual body we are willing to return.
/// Mirrors opencode's `isTextualMime` gate: `text/*`, plus the common
/// structured-text application types (json, xml, javascript, etc.). A binary
/// content-type (image, pdf, octet-stream) is refused.
fn is_textual_mime(content_type: &str) -> bool {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if ct.starts_with("text/") {
        return true;
    }
    matches!(
        ct.as_str(),
        "application/json"
            | "application/ld+json"
            | "application/xml"
            | "application/xhtml+xml"
            | "application/javascript"
            | "application/ecmascript"
            | "application/rss+xml"
            | "application/atom+xml"
    ) || ct.ends_with("+json")
        || ct.ends_with("+xml")
}

/// True when the content-type is HTML (drives the markdown conversion path).
fn is_html_mime(content_type: &str) -> bool {
    let ct = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    ct == "text/html" || ct == "application/xhtml+xml"
}

/// Convert a decoded textual body to the requested output format.
///
/// - `Html` → the raw body verbatim (no conversion, zero deps beyond the
///   textual gate).
/// - `Markdown` → HTML bodies run through `htmd`; already-textual non-HTML
///   bodies (plain text, markdown, json, xml) pass through raw.
///
/// A non-textual (binary) content-type is refused with
/// [`FetchError::UnsupportedContentType`] regardless of the requested format.
pub(crate) fn convert_body(
    body: &str,
    content_type: &str,
    format: FetchFormat,
) -> Result<String, FetchError> {
    if !is_textual_mime(content_type) {
        return Err(FetchError::UnsupportedContentType(content_type.to_string()));
    }
    match format {
        FetchFormat::Html => Ok(body.to_string()),
        FetchFormat::Markdown => {
            if is_html_mime(content_type) {
                htmd::convert(body).map_err(|e| FetchError::Decode(format!("html→markdown: {e}")))
            } else {
                // Non-HTML textual body (plain text, markdown, json, xml):
                // pass through raw — there is nothing to convert.
                Ok(body.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_markdown_via_htmd() {
        let md = convert_body(
            "<h1>Hello</h1>",
            "text/html; charset=utf-8",
            FetchFormat::Markdown,
        )
        .unwrap();
        assert!(md.contains("# Hello"));
    }

    #[test]
    fn html_format_returns_raw_body() {
        let raw = convert_body("<h1>Hi</h1>", "text/html", FetchFormat::Html).unwrap();
        assert_eq!(raw, "<h1>Hi</h1>");
    }

    #[test]
    fn plain_text_passthrough_markdown() {
        let out = convert_body("just text", "text/plain", FetchFormat::Markdown).unwrap();
        assert_eq!(out, "just text");
    }

    #[test]
    fn json_is_textual_passthrough() {
        let out = convert_body(r#"{"a":1}"#, "application/json", FetchFormat::Markdown).unwrap();
        assert_eq!(out, r#"{"a":1}"#);
    }

    #[test]
    fn suffix_json_is_textual() {
        assert!(is_textual_mime("application/vnd.api+json"));
        assert!(is_textual_mime("image/svg+xml"));
    }

    #[test]
    fn binary_content_type_refused() {
        for ct in ["image/png", "application/pdf", "application/octet-stream"] {
            assert!(matches!(
                convert_body("...", ct, FetchFormat::Markdown),
                Err(FetchError::UnsupportedContentType(_))
            ));
            // Refused even when raw HTML is requested.
            assert!(matches!(
                convert_body("...", ct, FetchFormat::Html),
                Err(FetchError::UnsupportedContentType(_))
            ));
        }
    }
}
