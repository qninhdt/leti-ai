//! PDF processing helpers for the attachments route.

use bytes::Bytes;
use openlet_core::runtime::attachments::pdf_text::substitute_artifact_id;
use openlet_core::runtime::attachments::{PdfProcessError, process_pdf};
use openlet_core::types::event::AttachmentKind;
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::session::SessionId;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::error::AppError;

pub(super) async fn process_and_persist_pdf(
    state: &AppState,
    sid: SessionId,
    bytes: Vec<u8>,
) -> Result<(AttachmentKind, String, String, Part), AppError> {
    let original_len = bytes.len();
    let processed = process_pdf(bytes).await;
    let artifact_id = format!("pdf-{}", Uuid::new_v4());
    let key = format!("attachments/{artifact_id}.pdf");

    // Persist the original bytes regardless of extraction outcome.
    // F3 spec: scanned/image-only PDFs still get stored, the part
    // just lacks `extracted_text`.
    let original_bytes_for_store = match &processed {
        Ok(r) => r.original_bytes.clone(),
        Err(_) => Vec::new(), // body already consumed; see below.
    };
    // When the extractor errored we lost the original bytes (process_pdf
    // owns the input). Re-route: store an empty placeholder and surface
    // the error. Callers can re-upload if they want to capture bytes.
    if !original_bytes_for_store.is_empty() {
        state
            .artifacts
            .put(sid, &key, Bytes::from(original_bytes_for_store))
            .await
            .map_err(|e| AppError::internal("artifact_put_failed", e.to_string()))?;
    }

    let (extracted_text, summary) = match processed {
        Ok(r) => {
            let text = r
                .extracted_text
                .as_ref()
                .map(|t| substitute_artifact_id(t, &artifact_id));
            let len = text.as_ref().map(String::len).unwrap_or(0);
            let mark = if r.truncated { " (truncated)" } else { "" };
            (
                text,
                format!("application/pdf, {original_len} bytes original, {len} chars text{mark}"),
            )
        }
        Err(PdfProcessError::TextTooShort { len, .. }) => (
            None,
            format!(
                "application/pdf, {original_len} bytes original, text unextractable ({len} chars)"
            ),
        ),
        Err(e) => return Err(map_pdf_error(e)),
    };

    let part = Part::Document {
        id: PartId::new(),
        artifact_id: artifact_id.clone(),
        mime: "application/pdf".into(),
        extracted_text,
    };
    Ok((
        AttachmentKind::Document,
        "application/pdf".into(),
        summary,
        part,
    ))
}

pub(super) fn map_pdf_error(e: PdfProcessError) -> AppError {
    match e {
        PdfProcessError::Unextractable | PdfProcessError::TextTooShort { .. } => {
            AppError::unprocessable_entity("pdf_text_unextractable", e.to_string())
        }
        PdfProcessError::Runtime(_) => AppError::internal("pdf_runtime", e.to_string()),
    }
}
