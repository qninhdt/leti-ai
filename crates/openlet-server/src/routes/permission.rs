//! `POST /v1/permission/:ask_id` — reply to a pending permission ask.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::permission::{
    AlwaysScope, AskId, PermissionAction, PermissionRule,
};
use openlet_protocol::{PermissionReplyDto, PermissionReplyKind};
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

    // Persist `always_*` rules BEFORE replying so any retry of the same
    // permission inside a tight tool batch already sees the new rule.
    // Scope is global today; the layered ruleset (§E) lands in 4C and will
    // pull session/agent scope from the original ask.
    if body.is_persistent() {
        let action = match body.decision {
            PermissionReplyKind::AlwaysAllow => PermissionAction::Allow,
            PermissionReplyKind::AlwaysDeny => PermissionAction::Deny,
            _ => unreachable!("is_persistent() guards the always_* variants"),
        };
        let pattern = body.pattern.clone().ok_or_else(|| {
            AppError::bad_request(
                "permission_pattern_required",
                "always_* decisions require `pattern` in the body",
            )
        })?;
        state
            .permission
            .record_always(
                AlwaysScope::Global,
                PermissionRule {
                    permission: pattern,
                    action,
                },
            )
            .await?;
    }

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
    Ok(StatusCode::OK)
}
