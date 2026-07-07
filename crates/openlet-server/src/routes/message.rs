//! `POST /v1/session/:id/prompt_async` — append user message + start turn.
//!
//! Fire-and-forget: spawns the runtime loop on a tokio task, returns
//! `202 Accepted` with the message id immediately. Errors propagate via
//! SSE `error` events, not the HTTP response.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::types::event::AgentEvent;
use openlet_core::runtime::PRESERVE_RECENT;
use openlet_core::types::message::{Message, MessageId, Role};
use openlet_core::types::part::Part;
use openlet_core::types::session::{SessionId, SessionStatus};
use openlet_protocol::{CompactAckDto, CreateMessageDto, MessageDto, PartDto, PromptAckDto};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app_state::{AppState, TurnHandle};
use crate::error::AppError;
use crate::events::publish_status;
use crate::mention::rewrite_mention_into_subagent_task;

/// Drop-guard that releases the `active_turns` slot if any `?` propagates
/// before we commit it to the spawned task. Once `committed = true`, the
/// driving task owns slot lifecycle (closes slot leak).
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
    // Terminal states (Errored / Cancelled) are RECOVERABLE: a fresh prompt
    // resumes the session rather than being rejected. Previously this returned
    // 409, so one failed turn left the session a permanent dead-end — the user
    // couldn't message again and had to start over. The `active_turns` slot
    // claim below is the real concurrency guard; a still-running turn is
    // rejected there as `turn_in_flight`. Re-prompting flips status back to
    // Running at the status update further down.

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
    // slot-leak on error path).
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
        // (closes stale-finalizer race).
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

#[utoipa::path(
    post,
    path = "/v1/session/{id}/compact",
    tag = "session",
    params(("id" = Uuid, Path, description = "Session id")),
    responses(
        (status = 202, description = "Compaction dispatched", body = CompactAckDto),
        (status = 404, description = "Session not found"),
        (status = 409, description = "A turn is already running"),
    )
)]
pub async fn compact(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<(StatusCode, Json<CompactAckDto>), AppError> {
    let sid = SessionId::from(id);
    let meta = state.require_session(sid).await?;

    // Nothing to compact when the conversation is at/under the preserved
    // floor — report it without claiming the turn slot or driving a model
    // turn, so `/compact` on a fresh session is a cheap no-op.
    let message_count = state.memory.list_messages(sid).await?.len();
    if message_count <= PRESERVE_RECENT {
        return Ok((StatusCode::ACCEPTED, Json(CompactAckDto { compacted: false })));
    }

    // Claim the active-turn slot before dispatch — compaction drives a
    // model turn, so it must not race a concurrent prompt for the session.
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

    state
        .memory
        .update_status(sid, SessionStatus::Running, "compact")
        .await?;
    publish_status(&state.events, sid, SessionStatus::Running).await;

    slot_guard.committed = true;
    drop(slot_guard);

    let task_state = state.clone();
    let task_handle = handle.clone();
    let agent_id = meta.agent_id;
    tokio::spawn(async move {
        struct ExitGuard(Arc<tokio::sync::Notify>);
        impl Drop for ExitGuard {
            fn drop(&mut self) {
                self.0.notify_waiters();
            }
        }
        let _exit_guard = ExitGuard(task_handle.exited.clone());

        let cancel = task_handle.cancel.clone();
        let outcome = drive_compaction(task_state.clone(), sid, agent_id, cancel.clone()).await;
        let final_status = match &outcome {
            Ok(_) => SessionStatus::Idle,
            Err(_) if cancel.is_cancelled() => SessionStatus::Cancelled,
            Err(_) => SessionStatus::Errored,
        };
        task_state.active_turns.remove_if(&sid, |_, h| {
            Arc::ptr_eq(&h.cancel_emitted, &task_handle.cancel_emitted)
        });
        let _ = task_state
            .memory
            .update_status(sid, final_status, status_reason(&outcome, &cancel))
            .await;
        publish_status(&task_state.events, sid, final_status).await;
        if let Err(err) = outcome {
            tracing::warn!(session = %sid, error = %err, "compaction ended with error");
        }
    });

    Ok((StatusCode::ACCEPTED, Json(CompactAckDto { compacted: true })))
}

/// Assemble the loop context and run a single on-demand compaction step.
async fn drive_compaction(
    state: AppState,
    session_id: SessionId,
    agent_id: openlet_core::types::agent::AgentId,
    cancel: CancellationToken,
) -> Result<(), openlet_core::error::CoreError> {
    let setup = crate::turn_driver::build_loop_context(&state, session_id, agent_id).await?;
    state
        .runtime
        .compact_session(&setup.memory, &setup.loop_ctx, setup.input, cancel)
        .await
        .map(|_| ())
}

#[utoipa::path(
    get,
    path = "/v1/session/{id}/messages",
    tag = "session",
    params(("id" = Uuid, Path, description = "Session id")),
    responses(
        (status = 200, description = "Messages with their parts, in append order", body = [MessageDto]),
        (status = 404, description = "Session not found"),
    )
)]
pub async fn list_messages(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<MessageDto>>, AppError> {
    let sid = SessionId::from(id);
    state.require_session(sid).await?;

    // Hydrate each message with its persisted parts (the Part tagged
    // union — text/reasoning/tool_call/tool_result/...). The streaming
    // protocol only carries part ids on part_created/part_updated, so a
    // resuming client fetches the full bodies here.
    let messages = state.memory.list_messages(sid).await?;
    let mut out = Vec::with_capacity(messages.len());
    for msg in messages {
        let parts = state.memory.list_parts(sid, msg.id).await?;
        let part_dtos: Vec<PartDto> = parts.into_iter().map(PartDto::from).collect();
        out.push(MessageDto::from_message(msg, part_dtos));
    }
    Ok(Json(out))
}

async fn drive_loop(
    state: AppState,
    session_id: SessionId,
    agent_id: openlet_core::types::agent::AgentId,
    cancel: CancellationToken,
) -> Result<(), openlet_core::error::CoreError> {
    let setup = crate::turn_driver::build_loop_context(&state, session_id, agent_id).await?;
    state
        .runtime
        .run_loop(&setup.memory, setup.loop_ctx, setup.input, cancel)
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
