//! PDF processing helpers for the attachments route.

use bytes::Bytes;
use leti_core::runtime::attachments::pdf_text::substitute_artifact_id;
use leti_core::runtime::attachments::{PdfProcessError, process_pdf};
use leti_core::types::event::AttachmentKind;
use leti_core::types::part::{Part, PartId};
use leti_core::types::session::SessionId;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::error::AppError;

pub(super) async fn process_and_persist_pdf(
    state: &AppState,
    sid: SessionId,
    bytes: Vec<u8>,
) -> Result<(AttachmentKind, String, String, String, Part), AppError> {
    let original_len = bytes.len();
    let processed = process_pdf(bytes).await;

    // Resolve to extractor output OR map the error to 422/500. We must
    // NOT fabricate an `artifact_id` and `Part::Document` referencing a
    // PDF the artifact store never received — every error variant of
    // `process_pdf` consumes the input bytes, so attempting to persist
    // an empty body would leave consumers with a 404 on the next
    // `artifact.get`.
    let r = processed.map_err(map_pdf_error)?;

    let artifact_id = format!("pdf-{}", Uuid::new_v4());
    let key = format!("attachments/{artifact_id}.pdf");

    state
        .artifacts
        .put(sid, &key, Bytes::from(r.original_bytes.clone()))
        .await
        .map_err(|e| AppError::internal("artifact_put_failed", e.to_string()))?;

    let extracted_text = r
        .extracted_text
        .as_ref()
        .map(|t| substitute_artifact_id(t, &artifact_id));
    let len = extracted_text.as_ref().map(String::len).unwrap_or(0);
    let mark = if r.truncated { " (truncated)" } else { "" };
    let summary = format!("application/pdf, {original_len} bytes original, {len} chars text{mark}");

    let part = Part::Document {
        id: PartId::new(),
        artifact_id: artifact_id.clone(),
        mime: "application/pdf".into(),
        extracted_text,
    };
    Ok((
        AttachmentKind::Document,
        "application/pdf".into(),
        artifact_id,
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
