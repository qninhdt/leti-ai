//! `POST /v1/session/:id/prompt_async` — append user message + start turn.
//!
//! Fire-and-forget: spawns the runtime loop on a tokio task, returns
//! `202 Accepted` with the message id immediately. Errors propagate via
//! SSE `error` events, not the HTTP response (per amendment §C / phase-05
//! plan step 4).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::projection::ProjectionCaps;
use openlet_core::runtime::LoopContext;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::message::{Message, MessageId, Role};
use openlet_core::types::part::Part;
use openlet_core::types::session::{SessionId, SessionStatus};
use openlet_protocol::{CreateMessageDto, PromptAckDto};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app_state::{AppState, TurnHandle};
use crate::error::AppError;
use crate::events::publish_status;
use crate::mention::rewrite_mention_into_subagent_task;

/// Drop-guard that releases the `active_turns` slot if any `?` propagates
/// before we commit it to the spawned task. Once `committed = true`, the
/// driving task owns slot lifecycle (closes C3-server slot leak).
struct SlotGuard<'a> {
    state: &'a AppState,
    sid: SessionId,
    committed: bool,
}

impl<'a> Drop for SlotGuard<'a> {
    fn drop(&mut self) {
        if !self.committed {
            self.state.active_turns.remove(&self.sid);
        }
    }
}

#[utoipa::path(
    post,
    path = "/v1/session/{id}/prompt_async",
    tag = "session",
    params(("id" = Uuid, Path, description = "Session id")),
    request_body = CreateMessageDto,
    responses(
        (status = 202, description = "Message accepted; turn dispatched", body = PromptAckDto),
        (status = 404, description = "Session not found"),
        (status = 409, description = "Session not in a runnable state"),
    )
)]
pub async fn prompt_async(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateMessageDto>,
) -> Result<(StatusCode, Json<PromptAckDto>), AppError> {
    let sid = SessionId::from(id);
    let meta = state.require_session(sid).await?;
    if matches!(
        meta.status,
        SessionStatus::Cancelled | SessionStatus::Errored
    ) {
        return Err(AppError::conflict(
            "session_terminal",
            format!("session is {:?}", meta.status),
        ));
    }

    let user_parts: Vec<Part> = body
        .parts
        .into_iter()
        .filter_map(|p| p.into_part_for_user_input())
        .collect();
    if user_parts.is_empty() {
        return Err(AppError::bad_request(
            "empty_message",
            "prompt_async requires at least one text or reasoning part",
        ));
    }

    // Mention rewrite — `@subagent_name objective…` at the start of a
    // text part rewrites into a synthetic `subagent_task` tool call.
    // The literal text part is preserved alongside so audit trails
    // still show what the user typed; the rewrite only adds the tool
    // call hint downstream tools can dispatch off.
    let user_parts = rewrite_mention_into_subagent_task(user_parts, &state);

    // Atomically claim the active-turn slot BEFORE we mutate session
    // state. `contains_key` then `insert` would let two concurrent
    // callers both pass and one would clobber the other, orphaning a
    // running task. The `SlotGuard` Drop releases the slot if any `?`
    // propagates before we commit it to the spawned task (closes
    // C3-server: slot-leak on error path).
    let handle = TurnHandle::new(sid);
    match state.active_turns.entry(sid) {
        dashmap::mapref::entry::Entry::Occupied(_) => {
            return Err(AppError::conflict(
                "turn_in_flight",
                "a turn is already running for this session",
            ));
        }
        dashmap::mapref::entry::Entry::Vacant(v) => {
            v.insert(handle.clone());
        }
    }
    let mut slot_guard = SlotGuard {
        state: &state,
        sid,
        committed: false,
    };

    let user_msg = Message {
        id: MessageId::new(),
        session_id: sid,
        role: Role::User,
        created_at: Utc::now(),
    };
    let user_msg_id = state.memory.append_message(sid, user_msg).await?;
    state
        .events
        .publish(
            AgentEvent::MessageCreated {
                session_id: sid,
                message_id: user_msg_id,
                at: Utc::now(),
            },
            Persistence::Durable,
        )
        .await?;
    for part in user_parts {
        let part_id = part.id();
        state.memory.append_part(user_msg_id, part).await?;
        state
            .events
            .publish(
                AgentEvent::PartCreated {
                    session_id: sid,
                    message_id: user_msg_id,
                    part_id,
                    at: Utc::now(),
                },
                Persistence::Durable,
            )
            .await?;
    }

    state
        .memory
        .update_status(sid, SessionStatus::Running, "prompt_async")
        .await?;
    publish_status(&state.events, sid, SessionStatus::Running).await;

    // All ?-propagating work is done. Commit the slot to the spawned
    // task — SlotGuard now drops without releasing.
    slot_guard.committed = true;
    drop(slot_guard);

    let task_state = state.clone();
    let task_handle = handle.clone();
    tokio::spawn(async move {
        // Drop guard ensures `exited` is notified on success, error, OR
        // panic. DELETE/abort awaiters resolve immediately on exit.
        struct ExitGuard(Arc<tokio::sync::Notify>);
        impl Drop for ExitGuard {
            fn drop(&mut self) {
                self.0.notify_waiters();
            }
        }
        let _exit_guard = ExitGuard(task_handle.exited.clone());

        let cancel = task_handle.cancel.clone();
        let outcome = drive_loop(task_state.clone(), sid, meta.agent_id, cancel.clone()).await;
        let final_status = match &outcome {
            Ok(_) => SessionStatus::Idle,
            Err(_) if cancel.is_cancelled() => SessionStatus::Cancelled,
            Err(_) => SessionStatus::Errored,
        };
        // Remove ONLY our own handle. If a fresh prompt_async raced past
        // a still-cancelling driver, this `remove_if` is a no-op so the
        // dying loop's tail finalizer can't stomp the new turn's slot
        // (closes C1-server stale-finalizer race).
        task_state.active_turns.remove_if(&sid, |_, h| {
            Arc::ptr_eq(&h.cancel_emitted, &task_handle.cancel_emitted)
        });
        let _ = task_state
            .memory
            .update_status(sid, final_status, status_reason(&outcome, &cancel))
            .await;
        publish_status(&task_state.events, sid, final_status).await;
        if let Err(err) = outcome {
            tracing::warn!(session = %sid, error = %err, "turn loop ended with error");
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(PromptAckDto {
            message_id: user_msg_id.as_uuid(),
            ack: true,
        }),
    ))
}

async fn drive_loop(
    state: AppState,
    session_id: SessionId,
    agent_id: openlet_core::types::agent::AgentId,
    cancel: CancellationToken,
) -> Result<(), openlet_core::error::CoreError> {
    let agent = state
        .agents
        .get(&agent_id)
        .ok_or(openlet_core::error::CoreError::Memory(
            openlet_core::error::MemoryError::SessionNotFound,
        ))?
        .clone();

    // Resolve provider capabilities by model so vision-capable
    // sessions don't degrade attachments to text. Uses the workspace's
    // configured default model — Phase 5 wires per-session model
    // overrides through SessionMeta.
    let model = state.config.default_model.clone();
    let provider_caps = state.provider.capabilities(&model);
    let projection_caps = ProjectionCaps {
        supports_reasoning_replay: false,
        supports_image_input: provider_caps.supports_vision,
        supports_document_input: provider_caps.supports_document_input,
    };
    let llm_messages =
        crate::turn_driver::project_session(&state, session_id, projection_caps).await?;

    let tools = crate::turn_driver::tool_specs(&state);

    let session_meta = state.memory.get_session(session_id).await?.ok_or(
        openlet_core::error::CoreError::Memory(openlet_core::error::MemoryError::SessionNotFound),
    )?;

    let read_history = state.read_histories.entry(session_id).or_default().clone();

    // Resolve the session's current profile slug — falls back to
    // `general` for legacy sessions that pre-date the
    // current_agent_slug column.
    let current_slug = session_meta
        .current_agent_slug
        .clone()
        .unwrap_or_else(|| "general".into());
    let agent_def = openlet_core::agent::AgentSlug::new(current_slug.clone())
        .ok()
        .and_then(|slug| state.agent_registry.get(&slug))
        .cloned()
        .map(std::sync::Arc::new);

    let loop_ctx = LoopContext {
        agent_id,
        fs: agent.fs.clone(),
        permission: state.permission.clone(),
        events: state.events.clone(),
        artifacts: state.artifacts.clone(),
        registry: state.tool_registry.clone(),
        read_history,
        mode: session_meta.permission_mode,
        max_steps: crate::turn_driver::MAX_TURN_STEPS,
        agent: agent_def,
        hook_chains: state.hook_chains.clone(),
        questions: state.questions.clone(),
        memory: state.memory.clone(),
        task_registry: state.task_registry.clone(),
        agent_registry: state.agent_registry.clone(),
    };

    let input = crate::turn_driver::build_turn_input(session_id, llm_messages, tools);

    let memory = crate::turn_driver::memory_arc(&state);
    state
        .runtime
        .run_loop(&memory, loop_ctx, input, cancel)
        .await
        .map(|_| ())
}

fn status_reason(
    outcome: &Result<(), openlet_core::error::CoreError>,
    cancel: &CancellationToken,
) -> &'static str {
    match outcome {
        Ok(_) => "turn finished",
        Err(_) if cancel.is_cancelled() => "cancelled",
        Err(_) => "loop error",
    }
}
