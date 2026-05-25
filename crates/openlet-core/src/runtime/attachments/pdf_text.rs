//! PDF text extraction pipeline.
//!
//! `pdf-extract` is a pure-Rust crate but its parser has historically
//! panicked on malformed PDFs. We layer two defenses:
//!   1. `tokio::task::spawn_blocking` so a slow parse doesn't stall
//!      the runtime.
//!   2. `panic::catch_unwind` inside the blocking closure so a parser
//!      panic returns `Unextractable` instead of crashing the worker.
//!      Requires workspace `panic = "unwind"` (verified at
//!      implementation start; see Cargo.toml — no `panic` key, default
//!      is unwind).
//!
//! Inlined text is capped at 50K characters (F3.10). The full text is
//! preserved on the result struct so the caller can decide whether to
//! also persist it as a separate artifact.

use std::panic::AssertUnwindSafe;

use thiserror::Error;

/// Minimum byte count of extracted text we treat as "real" content.
/// Anything shorter typically means the PDF is image-only (scanned)
/// or text-extraction failed silently. The caller still stores the
/// original bytes via `ArtifactStore`.
const MIN_USEFUL_TEXT_LEN: usize = 100;

/// Cap on inlined `extracted_text`. Beyond this we truncate with a
/// marker referencing the artifact id so the model can still reach
/// for the full text via a tool call. Closes F3.10.
pub const MAX_INLINE_TEXT_CHARS: usize = 50_000;

/// Successful extraction result. `extracted_text` is the truncated
/// projection (≤50K chars); `truncated` flags whether the source was
/// longer. `original_bytes` is moved through unchanged so the caller
/// can persist the original PDF exactly as received — never re-pickled
/// through `pdf-extract`.
#[derive(Debug, Clone)]
pub struct PdfProcessResult {
    pub original_bytes: Vec<u8>,
    pub extracted_text: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Error)]
pub enum PdfProcessError {
    #[error("pdf text unextractable (parser failure or scanned image)")]
    Unextractable,
    #[error("pdf extracted text too short ({len} < {min})")]
    TextTooShort { len: usize, min: usize },
    #[error("pdf runtime error: {0}")]
    Runtime(String),
}

/// Async entry point. Runs the parser inside a blocking worker with
/// `catch_unwind`. The original bytes are returned alongside the text
/// (or alongside an error) so the caller can persist the PDF
/// regardless of extraction outcome.
pub async fn process_pdf(input: Vec<u8>) -> Result<PdfProcessResult, PdfProcessError> {
    let bytes_for_parse = input.clone();
    let parsed = tokio::task::spawn_blocking(move || extract_text_catching(&bytes_for_parse))
        .await
        .map_err(|e| PdfProcessError::Runtime(e.to_string()))?;

    let text = match parsed {
        Ok(t) => t,
        Err(_) => return Err(PdfProcessError::Unextractable),
    };

    if text.len() < MIN_USEFUL_TEXT_LEN {
        return Err(PdfProcessError::TextTooShort {
            len: text.len(),
            min: MIN_USEFUL_TEXT_LEN,
        });
    }

    let (extracted_text, truncated) = truncate_inline_text(&text);
    Ok(PdfProcessResult {
        original_bytes: input,
        extracted_text: Some(extracted_text),
        truncated,
    })
}

/// Returns `Ok(text)` on a successful parse, `Err(())` on parser
/// error OR panic. Callers translate `Err(())` to `Unextractable`.
fn extract_text_catching(bytes: &[u8]) -> Result<String, ()> {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(bytes)
    }));
    match result {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(_)) | Err(_) => Err(()),
    }
}

/// Truncate `text` to ≤`MAX_INLINE_TEXT_CHARS` characters (NOT bytes),
/// appending a marker if anything was dropped. Returns
/// `(text, was_truncated)`.
///
/// The marker references the artifact id placeholder `<id>` so the
/// caller can substitute the real id once the artifact is persisted.
/// Keeping the placeholder static means the projection layer doesn't
/// need to know the artifact id at extraction time.
#[must_use]
pub fn truncate_inline_text(text: &str) -> (String, bool) {
    let char_count = text.chars().count();
    if char_count <= MAX_INLINE_TEXT_CHARS {
        return (text.to_string(), false);
    }
    let mut head: String = text.chars().take(MAX_INLINE_TEXT_CHARS).collect();
    head.push_str("\n\n[...truncated, full text in artifact <id>]");
    (head, true)
}

/// Re-stamp the truncation marker with a concrete artifact id once
/// the caller has persisted the PDF. Idempotent: safe to call when
/// `truncated` is false (returns the input unchanged).
#[must_use]
pub fn substitute_artifact_id(text: &str, artifact_id: &str) -> String {
    text.replace("<id>", artifact_id)
}
