//! `POST /v1/permission/:ask_id` — reply to a pending permission ask.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::permission::{AlwaysScope, AskId, PermissionAction};
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

    // Resolve session_id from the pending ask BEFORE consuming it so
    // the PermissionResolved event can be routed to the correct session.
    let session_id = state
        .permission
        .peek_session_id(ask)
        .ok_or_else(|| AppError::not_found("ask_not_found", "permission ask not found"))?;

    // For `always_*` decisions, use `accept_ask` — atomic take + persist
    // + push + resolve, with the rule pattern derived from the ORIGINAL
    // ask (never from client input). The action threads through so an
    // `always_deny` reply persists a Deny rule and resolves the in-flight
    // ask as Deny — the prior code unconditionally hardcoded Allow,
    // which silently inverted user intent.
    if body.is_persistent() {
        let scope = AlwaysScope::Global;
        let action = match body.decision {
            PermissionReplyKind::AlwaysAllow => PermissionAction::Allow,
            PermissionReplyKind::AlwaysDeny => PermissionAction::Deny,
            // Unreachable — is_persistent is true only for the two
            // Always* variants. Kept exhaustive for future enum growth.
            PermissionReplyKind::Allow | PermissionReplyKind::Deny => PermissionAction::Allow,
        };
        state
            .permission
            .accept_ask(ask, scope, action)
            .await
            .map_err(map_perm_err)?;
    } else {
        let decision = body.to_decision();
        state
            .permission
            .reply(ask, decision)
            .await
            .map_err(map_perm_err)?;
    }
    let resolved_decision = match body.decision {
        PermissionReplyKind::Allow | PermissionReplyKind::AlwaysAllow => {
            openlet_core::types::permission::Decision::Allow
        }
        PermissionReplyKind::Deny | PermissionReplyKind::AlwaysDeny => {
            openlet_core::types::permission::Decision::Deny {
                feedback: body.reason.clone(),
            }
        }
    };
    state
        .events
        .publish(
            AgentEvent::PermissionResolved {
                session_id,
                ask_id: ask,
                decision: resolved_decision,
            },
            Persistence::Durable,
        )
        .await?;
    Ok(StatusCode::OK)
}

fn map_perm_err(e: openlet_core::error::PermissionError) -> AppError {
    use openlet_core::error::PermissionError;
    match e {
        PermissionError::AskNotFound | PermissionError::AskExpired => {
            AppError::not_found("ask_not_found", "permission ask not found or expired")
        }
        PermissionError::Unsupported(s) => AppError::bad_request("unsupported_scope", s),
        e => AppError::from(e),
    }
}
