//! `POST /v1/session/:id/abort` — cancel the active turn.
//!
//! Per amendment §N: ack returns within 50ms, full teardown <500ms p95.
//! 1. Cancel the session token (synchronous).
//! 2. Spawn cleanup: mark status `Cancelling` + publish status event.
//!    Loop finalizer emits the eventual `Cancelled` once teardown lands.
//! 3. Return ack immediately — no awaited DB writes inline.

use axum::Json;
use axum::extract::{Path, State};
use chrono::Utc;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::session::{SessionId, SessionStatus};
use openlet_protocol::AbortAckDto;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::error::AppError;

#[utoipa::path(
    post,
    path = "/v1/session/{id}/abort",
    tag = "session",
    params(("id" = Uuid, Path, description = "Session id")),
    responses(
        (status = 200, description = "Abort acknowledged", body = AbortAckDto),
        (status = 404, description = "Session not found"),
    )
)]
pub async fn abort(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<AbortAckDto>, AppError> {
    let sid = SessionId::from(id);
    if state.memory.get_session(sid).await?.is_none() {
        return Err(AppError::not_found(
            "session_not_found",
            "session not found",
        ));
    }

    let aborted = if let Some((_, handle)) = state.active_turns.remove(&sid) {
        handle.cancel.cancel();
        true
    } else {
        false
    };

    let cleanup_state = state.clone();
    tokio::spawn(async move {
        if let Err(err) = cleanup_state
            .memory
            .update_status(sid, SessionStatus::Cancelling, "client abort")
            .await
        {
            tracing::warn!(session = %sid, error = %err, "abort cleanup: status write failed");
        }
        if let Err(err) = cleanup_state
            .events
            .publish(
                AgentEvent::SessionStatus {
                    session_id: sid,
                    status: SessionStatus::Cancelling,
                    at: Utc::now(),
                },
                Persistence::Durable,
            )
            .await
        {
            tracing::warn!(session = %sid, error = %err, "abort cleanup: event publish failed");
        }
    });

    Ok(Json(AbortAckDto { aborted }))
}
