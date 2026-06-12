//! `POST /v1/session/:id/attachments` — multipart upload route.
//!
//! Pipeline:
//!   1. Body limit layer (`RequestBodyLimitLayer::new(25MB)`) — applied
//!      to this route specifically because axum's `DefaultBodyLimit`
//!      caps at 2MB.
//!   2. `axum::extract::Multipart` reads the `file` field.
//!   3. `infer` content-sniffs the bytes. Reject when no MIME or when
//!      magic bytes claim two formats (polyglot).
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

mod image;
mod pdf;
mod sniff;

use axum::Json;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::types::event::{AgentEvent, AttachmentKind};
use openlet_core::types::message::{Message, MessageId, Role};
use openlet_core::types::session::SessionId;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::error::AppError;

use self::image::process_and_persist_image;
use self::pdf::process_and_persist_pdf;
use self::sniff::{DetectedKind, sniff_content_type};

/// Hard cap on the multipart body. Larger uploads are short-circuited
/// at the tower layer and never reach this handler.
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
    path = "/v1/session/{id}/attachments",
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

    // Content-sniff before processing. The multipart
    // `Content-Type` claim is treated as a hint only; `infer` walks
    // the magic bytes.
    let detected = sniff_content_type(&file_bytes)?;

    let (kind, mime, artifact_id, summary, part) = match detected {
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
                artifact_id: artifact_id.clone(),
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
            artifact_id,
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
/// `RouterBuilder::build` doesn't fire first.
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
