//! `POST /v1/session/:id/abort` — cancel the active turn.
//!
//! Per amendment §N: ack returns within 50ms, full teardown <500ms p95.
//! 1. Cancel the session token (synchronous).
//! 2. Spawn cleanup: mark status `Cancelling`. Loop finalizer emits the
//!    eventual `Cancelled` once teardown lands.
//! 3. Return ack immediately — no awaited DB writes inline.

use axum::Json;
use axum::extract::{Path, State};
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
    let _ = state.require_session(sid).await?;

    // Don't remove the slot here — let the driving task remove its own
    // handle on exit (closes C1-server stale-finalizer race). The shared
    // `try_cancel_active_turn` helper trips the cancel token via the CAS
    // gate AND publishes the `Cancelling` event so concurrent abort +
    // DELETE + cancel_session emit exactly one event.
    let aborted = state.try_cancel_active_turn(sid).await;
    if aborted {
        let cleanup_state = state.clone();
        tokio::spawn(async move {
            if let Err(err) = cleanup_state
                .memory
                .update_status(sid, SessionStatus::Cancelling, "client abort")
                .await
            {
                tracing::warn!(session = %sid, error = %err, "abort cleanup: status write failed");
            }
        });
    }

    Ok(Json(AbortAckDto { aborted }))
}
