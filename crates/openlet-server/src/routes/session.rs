//! `/v1/session` — CRUD + permission mode.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::runtime::question_registry::QuestionRegistry;
use openlet_core::runtime::subagent::{BackgroundTransition, TaskId};
use openlet_core::tools::ReadHistory;
use openlet_core::tools::builtins::subagent_task::SubagentSpawner;
use openlet_core::types::agent::AgentId;
use openlet_core::types::message::MessageId;
use openlet_core::types::session::{SessionCapabilities, SessionFilter, SessionId, SessionMeta};
use openlet_protocol::ContinueSubagentDto;
use openlet_protocol::{
    BackgroundTaskAckDto, CreateSessionDto, SessionDto, SetModeDto, SubagentControlAckDto,
    SubagentExecutionDto,
};
use uuid::Uuid;

use crate::app_state::AppState;
use crate::error::AppError;
use crate::events::publish_status;

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
    // Build the row explicitly so the caller-declared capabilities +
    // permission mode are honored. `create_session` (the bare path) hardcodes
    // all-false capabilities and the default mode; the interactive create route
    // must instead enable `user_questions` so `ask_user` doesn't fail fast.
    let capabilities = SessionCapabilities {
        user_questions: body.user_questions,
    };
    let meta = SessionMeta::new_root(
        SessionId::new(),
        agent_id,
        parent,
        body.permission_mode.unwrap_or_default(),
        capabilities,
        chrono::Utc::now(),
    );
    let id = state.memory.create_session_with_meta(meta).await?;
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
    let meta = state.require_session(SessionId::from(id)).await?;
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
    // Cancel any in-flight turn BEFORE marking the session terminal so
    // the LLM can't keep streaming on a session the client thinks is
    // gone. Idempotent via CAS gate.
    let exit_notify = state.active_turns.get(&sid).map(|h| h.exited.clone());
    let _ = state.try_cancel_active_turn(sid).await;
    // A root can own background children after its foreground turn has
    // returned. Deleting it must cascade through those independent task
    // tokens as well, otherwise a detached child could settle and try to
    // deliver into a deleted parent session.
    state.task_registry.cancel_descendants(sid);
    if let Some(exited) = exit_notify {
        // Wait for the driving task's Drop guard to signal exit. Notify
        // permits-on-await semantics: if the task already exited, this
        // resolves immediately the next loop iteration.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), exited.notified()).await;
    }
    state.memory.delete_session(sid).await?;
    // Drop the cumulative-cost entry now the session is gone — DELETE is
    // the true terminal (Idle/Errored are resumable), so this is the one
    // place it's safe to evict without losing mid-conversation totals.
    state.runtime.evict_session_cost(sid);
    publish_status(
        &state.events,
        sid,
        openlet_core::types::session::SessionStatus::Cancelled,
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

/// `POST /v1/session/:id/task/:task_id/background` — detach a running
/// foreground task without restarting its child session. The registry's CAS
/// decides whether the task or a concurrent terminal settlement owns output.
#[utoipa::path(
    post,
    path = "/v1/session/{id}/task/{task_id}/background",
    tag = "session",
    params(
        ("id" = Uuid, Path, description = "Parent session id"),
        ("task_id" = Uuid, Path, description = "Subagent task id"),
    ),
    responses(
        (status = 200, description = "Task is or was backgrounded", body = BackgroundTaskAckDto),
        (status = 404, description = "Session or task not found"),
        (status = 409, description = "Task already settled"),
    )
)]
pub async fn background_task(
    State(state): State<AppState>,
    Path((id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<BackgroundTaskAckDto>, AppError> {
    let parent_session_id = SessionId::from(id);
    let _ = state.require_session(parent_session_id).await?;
    match state
        .task_registry
        .background_task(TaskId(task_id), parent_session_id)
    {
        BackgroundTransition::Backgrounded | BackgroundTransition::AlreadyBackground => {
            Ok(Json(BackgroundTaskAckDto {
                task_id,
                status: "running".to_string(),
            }))
        }
        BackgroundTransition::AlreadyTerminal => Err(AppError::conflict(
            "task_already_settled",
            "task settled before it could be backgrounded",
        )),
        BackgroundTransition::NotFound => Err(AppError::not_found(
            "task_not_found",
            "running task not found under this parent session",
        )),
    }
}

#[utoipa::path(
    get,
    path = "/v1/session/{id}/subagents",
    tag = "session",
    params(("id" = Uuid, Path, description = "Root or descendant session id")),
    responses((status = 200, description = "Durable subagent executions", body = [SubagentExecutionDto]))
)]
pub async fn list_subagents(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<SubagentExecutionDto>>, AppError> {
    let root = root_session(&state, SessionId::from(id)).await?;
    let executions = state.memory.list_subagent_executions(root, true).await?;
    Ok(Json(executions.into_iter().map(Into::into).collect()))
}

#[utoipa::path(
    post,
    path = "/v1/session/{id}/subagent/{task_id}/cancel",
    tag = "session",
    params(("id" = Uuid, Path, description = "Root or descendant session id"), ("task_id" = Uuid, Path, description = "Subagent execution id")),
    responses((status = 200, description = "Cancellation requested", body = SubagentControlAckDto), (status = 404, description = "Task not found"))
)]
pub async fn cancel_subagent(
    State(state): State<AppState>,
    Path((id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<SubagentControlAckDto>, AppError> {
    control_subagent(&state, SessionId::from(id), TaskId(task_id), false).await
}

#[utoipa::path(
    post,
    path = "/v1/session/{id}/subagent/{task_id}/interrupt",
    tag = "session",
    params(("id" = Uuid, Path, description = "Root or descendant session id"), ("task_id" = Uuid, Path, description = "Subagent execution id")),
    responses((status = 200, description = "Interruption requested", body = SubagentControlAckDto), (status = 404, description = "Task not found"))
)]
pub async fn interrupt_subagent(
    State(state): State<AppState>,
    Path((id, task_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<SubagentControlAckDto>, AppError> {
    control_subagent(&state, SessionId::from(id), TaskId(task_id), true).await
}

/// Start a new execution against an existing child session. A short-lived
/// runtime spawner is sufficient here: task ownership itself is durable in
/// SQLite and the driver only needs the shared `AppState` handles.
#[utoipa::path(
    post,
    path = "/v1/session/{id}/subagent/continue",
    tag = "session",
    params(("id" = Uuid, Path, description = "Root or descendant session id")),
    request_body = ContinueSubagentDto,
    responses((status = 202, description = "Continuation accepted", body = SubagentControlAckDto), (status = 404, description = "Session or child not found"), (status = 409, description = "Child already has a live execution"))
)]
pub async fn continue_subagent(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ContinueSubagentDto>,
) -> Result<(StatusCode, Json<SubagentControlAckDto>), AppError> {
    let requester = SessionId::from(id);
    let root = root_session(&state, requester).await?;
    let child_session_id = SessionId::from(body.child_session_id);
    let child = state.require_session(child_session_id).await?;
    let child_root = root_session(&state, child_session_id).await?;
    if child_root != root || child.parent_session_id.is_none() {
        return Err(AppError::not_found(
            "subagent_child_not_found",
            "subagent child not found",
        ));
    }
    let resources = state
        .agents
        .get(&state.require_session(root).await?.agent_id)
        .ok_or_else(|| AppError::not_found("agent_not_found", "root agent not registered"))?;
    let ctx = ToolCtx {
        session_id: root,
        agent_id: resources.spec.id,
        message_id: MessageId::new(),
        call_id: "http-subagent-continue".into(),
        mode: state.require_session(root).await?.permission_mode,
        fs: resources.fs.clone(),
        permission: state.permission.clone(),
        events: state.events.clone(),
        artifacts: state.artifacts.clone(),
        read_history: ReadHistory::new(),
        cancel: tokio_util::sync::CancellationToken::new(),
        questions: Arc::new(QuestionRegistry::new()),
        memory: state.memory.clone(),
        task_registry: state.task_registry.clone(),
        agent_registry: state.agent_registry.clone(),
    };
    let spawner = crate::subagent_spawner::RuntimeSubagentSpawner::new();
    spawner.set_state(state.clone());
    let spawned = spawner
        .continue_subagent(&ctx, child_session_id, &body.objective, body.background)
        .await
        .map_err(|error| AppError::conflict(error.code(), error.to_string()))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(SubagentControlAckDto {
            task_id: spawned.task_id.0,
            status: "running".to_string(),
        }),
    ))
}

async fn control_subagent(
    state: &AppState,
    requester: SessionId,
    task_id: TaskId,
    interrupt: bool,
) -> Result<Json<SubagentControlAckDto>, AppError> {
    let root = root_session(state, requester).await?;
    let execution = state
        .memory
        .get_subagent_execution(task_id)
        .await?
        .ok_or_else(|| {
            AppError::not_found("subagent_task_not_found", "subagent execution not found")
        })?;
    if execution.root_session_id != root {
        return Err(AppError::not_found(
            "subagent_task_not_found",
            "subagent execution not found",
        ));
    }
    if !execution.status.is_terminal() {
        if interrupt {
            state.task_registry.interrupt(task_id);
        } else {
            state.task_registry.cancel(task_id);
        }
    }
    Ok(Json(SubagentControlAckDto {
        task_id: task_id.0,
        status: if execution.status.is_terminal() {
            execution.status.label().to_string()
        } else if interrupt {
            "interrupting".to_string()
        } else {
            "cancelling".to_string()
        },
    }))
}

async fn root_session(state: &AppState, session: SessionId) -> Result<SessionId, AppError> {
    let mut current = session;
    for _ in 0..8 {
        let meta = state.require_session(current).await?;
        match meta.parent_session_id {
            Some(parent) => current = parent,
            None => return Ok(current),
        }
    }
    Err(AppError::conflict(
        "subagent_parent_cycle",
        "session parent chain exceeds maximum depth",
    ))
}
