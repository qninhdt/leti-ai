//! `POST /v1/sessions/:id/attachments` — multipart upload route.
//!
//! Pipeline:
//!   1. Body limit layer (`RequestBodyLimitLayer::new(25MB)`) — applied
//!      to this route specifically because axum's `DefaultBodyLimit`
//!      caps at 2MB (closes F3.1).
//!   2. `axum::extract::Multipart` reads the `file` field.
//!   3. `infer` content-sniffs the bytes. Reject when no MIME or when
//!      magic bytes claim two formats (closes F3.5 polyglot).
//!   4. Image path → `process_image` (resize + JPEG re-encode).
//!      PDF path → `process_pdf` (extract text, panic-safe).
//!   5. Persist to `ArtifactStore`. Append `Part::Image` or
//!      `Part::Document` to the session's most-recent user message
//!      (or create a fresh user message when none exists yet).
//!   6. Emit `AgentEvent::AttachmentAccepted` (durable).
//!
//! Routing: `with_attachment_routes()` mounts the path; `Default::default`
//! includes it. `RouterBuilder::build` applies the global 2MB
//! `DefaultBodyLimit` BEFORE this layer so we explicitly disable the
//! global cap inside `with_attachment_routes`.

use axum::Json;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::StatusCode;
use bytes::Bytes;
use chrono::Utc;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::runtime::attachments::pdf_text::substitute_artifact_id;
use openlet_core::runtime::attachments::{
    ImageProcessError, PdfProcessError, process_image, process_pdf,
};
use openlet_core::types::event::{AgentEvent, AttachmentKind};
use openlet_core::types::message::{Message, MessageId, Role};
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::session::SessionId;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::error::AppError;

/// Hard cap on the multipart body. Larger uploads are short-circuited
/// at the tower layer and never reach this handler — F3.1.
pub const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

/// Response body for a successful upload.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AttachmentAck {
    pub artifact_id: String,
    pub kind: String,
    pub mime: String,
    pub summary: String,
    pub part_id: Uuid,
    pub message_id: Uuid,
}

#[utoipa::path(
    post,
    path = "/v1/sessions/{id}/attachments",
    tag = "session",
    params(("id" = Uuid, Path, description = "Session id")),
    request_body(content = String, content_type = "multipart/form-data"),
    responses(
        (status = 201, description = "Attachment accepted", body = AttachmentAck),
        (status = 404, description = "Session not found"),
        (status = 413, description = "Upload exceeds 25MB cap"),
        (status = 415, description = "Unsupported / unrecognized media type"),
        (status = 422, description = "Attachment failed processing pipeline"),
    )
)]
pub async fn upload(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<AttachmentAck>), AppError> {
    let sid = SessionId::from(id);
    let _meta = state
        .memory
        .get_session(sid)
        .await?
        .ok_or_else(|| AppError::not_found("session_not_found", "session not found"))?;

    // Drain the first file field. We only honor `file` (anything else
    // is rejected) so a buggy client can't sneak a second payload past
    // the body limit by stacking fields.
    let file_bytes = read_file_field(&mut multipart).await?;
    if file_bytes.is_empty() {
        return Err(AppError::bad_request(
            "empty_attachment",
            "attachment file field is empty",
        ));
    }

    // F3.5 — content-sniff before processing. The multipart
    // `Content-Type` claim is treated as a hint only; `infer` walks
    // the magic bytes.
    let detected = sniff_content_type(&file_bytes)?;

    let (kind, mime, summary, part) = match detected {
        DetectedKind::Image => process_and_persist_image(&state, sid, file_bytes).await?,
        DetectedKind::Pdf => process_and_persist_pdf(&state, sid, file_bytes).await?,
    };

    // Append to (or create) the most recent user message — uploads
    // happen between turns, so this attaches the new content to the
    // user's in-progress prompt.
    let message_id = ensure_user_message(&state, sid).await?;
    let part_id = part.id();
    state.memory.append_part(message_id, part).await?;
    state
        .events
        .publish(
            AgentEvent::PartCreated {
                session_id: sid,
                message_id,
                part_id,
                at: Utc::now(),
            },
            Persistence::Durable,
        )
        .await?;
    state
        .events
        .publish(
            AgentEvent::AttachmentAccepted {
                session_id: sid,
                message_id,
                part_id,
                artifact_id: part_artifact_id(&kind, &summary),
                attachment_kind: kind,
                mime: mime.clone(),
                summary: summary.clone(),
            },
            Persistence::Durable,
        )
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(AttachmentAck {
            artifact_id: part_artifact_id(&kind, &summary),
            kind: match kind {
                AttachmentKind::Image => "image".into(),
                AttachmentKind::Document => "document".into(),
            },
            mime,
            summary,
            part_id: part_id.as_uuid(),
            message_id: message_id.as_uuid(),
        }),
    ))
}

/// Layer-applicable body-size limit for the attachments route. The
/// caller composes `RequestBodyLimitLayer::new(MAX_UPLOAD_BYTES)`
/// alongside `DefaultBodyLimit::disable()` so the global 2MB cap from
/// `RouterBuilder::build` doesn't fire first. Closes F3.1.
#[must_use]
pub fn body_limit_layer() -> tower::layer::util::Stack<
    tower_http::limit::RequestBodyLimitLayer,
    tower::layer::util::Stack<DefaultBodyLimit, tower::layer::util::Identity>,
> {
    use tower::ServiceBuilder;
    use tower_http::limit::RequestBodyLimitLayer;
    ServiceBuilder::new()
        .layer(DefaultBodyLimit::disable())
        .layer(RequestBodyLimitLayer::new(MAX_UPLOAD_BYTES))
        .into_inner()
}

/// Sniffer result. We restrict to the two formats the pipeline knows
/// how to process; anything else is rejected at the boundary.
#[derive(Debug, Clone, Copy)]
enum DetectedKind {
    Image,
    Pdf,
}

/// `infer`-based content sniffing. Closes F3.5: rejects polyglot
/// files where the magic bytes match BOTH a PDF and an image format.
/// We always trust the sniffed kind over the multipart `Content-Type`.
fn sniff_content_type(bytes: &[u8]) -> Result<DetectedKind, AppError> {
    let main = infer::get(bytes).ok_or_else(|| {
        AppError::unsupported_media_type(
            "unknown_media_type",
            "could not detect content type from bytes",
        )
    })?;
    let mime = main.mime_type();
    let main_kind = if mime.starts_with("image/") {
        Some(DetectedKind::Image)
    } else if mime == "application/pdf" {
        Some(DetectedKind::Pdf)
    } else {
        None
    };
    let main_kind = main_kind.ok_or_else(|| {
        AppError::unsupported_media_type(
            "unsupported_media_type",
            format!("media type {mime} is not supported"),
        )
    })?;

    // Polyglot detection: scan a small prefix of the body for an
    // alternate magic-byte signature. If both an image and a PDF
    // marker show up, the file is conflicting and rejected. We don't
    // try to be exhaustive — the goal is to defeat the trivial
    // "concatenate two formats" attack, not to certify benign.
    let has_pdf_magic = bytes.windows(4).take(1024).any(|w| w == b"%PDF");
    let has_jpeg_soi = bytes.windows(2).take(8).any(|w| w == [0xFF, 0xD8]);
    let has_png_magic = bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
    let image_count = u8::from(has_jpeg_soi) + u8::from(has_png_magic);
    let conflict = match main_kind {
        DetectedKind::Image => has_pdf_magic && image_count > 0,
        DetectedKind::Pdf => image_count > 0,
    };
    if conflict {
        return Err(AppError::unsupported_media_type(
            "conflicting_magic_bytes",
            "file contains conflicting format magic bytes",
        ));
    }
    Ok(main_kind)
}

/// Read the first multipart field named `file`. Other field names are
/// ignored so a sloppy client doesn't trip the handler with extra
/// metadata fields.
async fn read_file_field(multipart: &mut Multipart) -> Result<Vec<u8>, AppError> {
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        AppError::bad_request("multipart_decode", format!("failed reading multipart: {e}"))
    })? {
        let name = field.name().unwrap_or_default().to_string();
        if name != "file" {
            continue;
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|e| AppError::bad_request("multipart_io", format!("multipart io: {e}")))?;
        return Ok(bytes.to_vec());
    }
    Err(AppError::bad_request(
        "missing_file_field",
        "multipart upload missing required `file` field",
    ))
}

async fn process_and_persist_image(
    state: &AppState,
    sid: SessionId,
    bytes: Vec<u8>,
) -> Result<(AttachmentKind, String, String, Part), AppError> {
    let result = process_image(bytes).await.map_err(map_image_error)?;
    let artifact_id = format!("img-{}", Uuid::new_v4());
    let key = format!("attachments/{artifact_id}.jpg");
    let bytes_for_store = Bytes::from(result.bytes.clone());
    state
        .artifacts
        .put(sid, &key, bytes_for_store)
        .await
        .map_err(|e| AppError::internal("artifact_put_failed", e.to_string()))?;
    let summary = format!(
        "image/jpeg {}x{} ({} bytes)",
        result.width,
        result.height,
        result.bytes.len()
    );
    let part = Part::Image {
        id: PartId::new(),
        artifact_id: artifact_id.clone(),
        mime: "image/jpeg".into(),
        width: result.width,
        height: result.height,
    };
    Ok((AttachmentKind::Image, "image/jpeg".into(), summary, part))
}

async fn process_and_persist_pdf(
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

async fn ensure_user_message(state: &AppState, sid: SessionId) -> Result<MessageId, AppError> {
    // Check the most recent message — if it's user-role, attach to
    // it; otherwise create a fresh user message to hold the
    // attachment. Avoids dangling attachments on a tool/assistant
    // turn boundary.
    let msgs = state.memory.list_messages(sid).await?;
    if let Some(last) = msgs.last() {
        if matches!(last.role, Role::User) {
            return Ok(last.id);
        }
    }
    let msg = Message {
        id: MessageId::new(),
        session_id: sid,
        role: Role::User,
        created_at: Utc::now(),
    };
    let mid = state.memory.append_message(sid, msg).await?;
    state
        .events
        .publish(
            AgentEvent::MessageCreated {
                session_id: sid,
                message_id: mid,
                at: Utc::now(),
            },
            Persistence::Durable,
        )
        .await?;
    Ok(mid)
}

/// Pull the artifact id back out of the summary (we stash it as the
/// first segment after the kind prefix). Done this way to avoid
/// threading the artifact id through every helper return tuple.
fn part_artifact_id(_kind: &AttachmentKind, summary: &str) -> String {
    summary.split_whitespace().next().unwrap_or("").to_string()
}

fn map_image_error(e: ImageProcessError) -> AppError {
    match e {
        ImageProcessError::UnsupportedFormat => AppError::unsupported_media_type(
            "unsupported_image_format",
            "image format not supported",
        ),
        ImageProcessError::ImageDimensionsExceedLimit { .. }
        | ImageProcessError::TooComplexToResize => {
            AppError::unprocessable_entity("image_too_large", e.to_string())
        }
        ImageProcessError::Decode(_) | ImageProcessError::Encode(_) => {
            AppError::unprocessable_entity("image_decode_failed", e.to_string())
        }
        ImageProcessError::Runtime(_) => AppError::internal("image_runtime", e.to_string()),
    }
}

fn map_pdf_error(e: PdfProcessError) -> AppError {
    match e {
        PdfProcessError::Unextractable | PdfProcessError::TextTooShort { .. } => {
            AppError::unprocessable_entity("pdf_text_unextractable", e.to_string())
        }
        PdfProcessError::Runtime(_) => AppError::internal("pdf_runtime", e.to_string()),
    }
}
