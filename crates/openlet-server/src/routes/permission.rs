//! `POST /v1/permission/:ask_id` — reply to a pending permission ask.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::permission::AskId;
use openlet_protocol::PermissionReplyDto;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::error::AppError;

#[utoipa::path(
    post,
    path = "/v1/permission/{ask_id}",
    tag = "permission",
    params(("ask_id" = Uuid, Path, description = "Pending ask id")),
    request_body = PermissionReplyDto,
    responses(
        (status = 200, description = "Reply accepted"),
        (status = 404, description = "Ask not found"),
    )
)]
pub async fn reply(
    State(state): State<AppState>,
    Path(ask_id): Path<Uuid>,
    Json(body): Json<PermissionReplyDto>,
) -> Result<StatusCode, AppError> {
    let ask = AskId(ask_id);
    let decision = body.to_decision();
    state.permission.reply(ask, decision.clone()).await?;
    state
        .events
        .publish(
            AgentEvent::PermissionResolved {
                ask_id: ask,
                decision,
            },
            Persistence::Durable,
        )
        .await?;
    // `always_*` rule persistence is owned by the manager via
    // `record_always`; we leave that wiring to phase-08 hardening since
    // the route already exposes the binary outcome runtime needs.
    Ok(StatusCode::OK)
}
