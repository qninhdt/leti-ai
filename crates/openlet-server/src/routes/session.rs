//! `/v1/session` — CRUD + permission mode.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::session::{SessionFilter, SessionId};
use openlet_protocol::{CreateSessionDto, SessionDto, SetModeDto};
use uuid::Uuid;

use crate::app_state::AppState;
use crate::error::AppError;

/// `POST /v1/session` — create session.
#[utoipa::path(
    post,
    path = "/v1/session",
    tag = "session",
    request_body = CreateSessionDto,
    responses(
        (status = 201, description = "Session created", body = SessionDto),
        (status = 400, description = "Invalid request"),
    )
)]
pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateSessionDto>,
) -> Result<(StatusCode, Json<SessionDto>), AppError> {
    let agent_id = body
        .agent_id
        .map(AgentId::from)
        .unwrap_or(state.default_agent_id);
    if !state.agents.contains_key(&agent_id) {
        return Err(AppError::not_found(
            "agent_not_found",
            format!("agent {agent_id} not registered"),
        ));
    }
    let parent = body.parent_session_id.map(SessionId::from);
    let id = state.memory.create_session(agent_id, parent).await?;
    if !body.extensions.is_null() {
        state
            .memory
            .update_session_extensions(id, body.extensions)
            .await?;
    }
    let meta = state
        .memory
        .get_session(id)
        .await?
        .ok_or_else(|| AppError::internal("session_lost", "session vanished after create"))?;
    Ok((StatusCode::CREATED, Json(SessionDto::from(meta))))
}

/// `GET /v1/session` — list sessions (excluding deleted by default).
#[utoipa::path(
    get,
    path = "/v1/session",
    tag = "session",
    responses(
        (status = 200, description = "Sessions", body = [SessionDto])
    )
)]
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<SessionDto>>, AppError> {
    let metas = state.memory.list_sessions(SessionFilter::default()).await?;
    Ok(Json(metas.into_iter().map(SessionDto::from).collect()))
}

/// `GET /v1/session/:id` — fetch one session.
#[utoipa::path(
    get,
    path = "/v1/session/{id}",
    tag = "session",
    params(("id" = Uuid, Path, description = "Session id")),
    responses(
        (status = 200, description = "Session", body = SessionDto),
        (status = 404, description = "Not found"),
    )
)]
pub async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<SessionDto>, AppError> {
    let meta = state
        .memory
        .get_session(SessionId::from(id))
        .await?
        .ok_or_else(|| AppError::not_found("session_not_found", "session not found"))?;
    Ok(Json(SessionDto::from(meta)))
}

/// `DELETE /v1/session/:id` — soft-delete.
#[utoipa::path(
    delete,
    path = "/v1/session/{id}",
    tag = "session",
    params(("id" = Uuid, Path, description = "Session id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found"),
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let sid = SessionId::from(id);
    state.memory.delete_session(sid).await?;
    let _ = state
        .events
        .publish(
            AgentEvent::SessionStatus {
                session_id: sid,
                status: openlet_core::types::session::SessionStatus::Cancelled,
                at: Utc::now(),
            },
            Persistence::Durable,
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/session/:id/mode` — change permission mode.
#[utoipa::path(
    post,
    path = "/v1/session/{id}/mode",
    tag = "session",
    params(("id" = Uuid, Path, description = "Session id")),
    request_body = SetModeDto,
    responses(
        (status = 200, description = "Updated", body = SessionDto),
        (status = 404, description = "Not found"),
    )
)]
pub async fn set_mode(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetModeDto>,
) -> Result<Json<SessionDto>, AppError> {
    let sid = SessionId::from(id);
    state.memory.update_permission_mode(sid, body.mode).await?;
    let meta = state
        .memory
        .get_session(sid)
        .await?
        .ok_or_else(|| AppError::not_found("session_not_found", "session not found"))?;
    Ok(Json(SessionDto::from(meta)))
}
