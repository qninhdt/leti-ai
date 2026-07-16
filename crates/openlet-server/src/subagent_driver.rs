//! Subagent driver task.
//!
//! Runs a nested `ConversationRuntime::run_loop` on a child session,
//! mirrors output + cost into the task registry, and emits the terminal
//! `SubagentSettled` event. Extracted from `subagent_spawner.rs` to keep
//! the spawner focused on admission + task lifecycle.

use std::sync::Arc;

use openlet_core::adapters::event_sink::Persistence;
use openlet_core::adapters::memory_store::SubagentExecutionPatch;
use openlet_core::error::CoreError;
use openlet_core::projection::ProjectionCaps;
use openlet_core::runtime::LoopContext;
use openlet_core::runtime::subagent::{
    DeliveryOwnership, SubagentExecutionStatus, TaskId, TaskStatus,
};
use openlet_core::types::event::AgentEvent;
use openlet_core::types::part::Part;
use openlet_core::types::session::SessionId;

use crate::app_state::AppState;

/// Driver task — runs a nested `ConversationRuntime::run_loop` on the
/// child session, mirrors output + cost into the task registry, and
/// emits the terminal `SubagentSettled` event.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_subagent(
    state: AppState,
    task_id: TaskId,
    parent_session_id: SessionId,
    child_session_id: SessionId,
    agent_slug: openlet_core::agent::AgentSlug,
    child_perm: Arc<dyn openlet_core::adapters::permission_manager::PermissionManager>,
    child_cancel: tokio_util::sync::CancellationToken,
    agent_resources: crate::app_state::AgentResources,
    // Phase 4 roster identity: the root session this task is rostered under
    // and the unique handle name assigned at spawn, so the driver removes
    // the roster entry when the task is no longer background-alive.
    root_session_id: SessionId,
    handle_name: openlet_core::runtime::subagent::HandleName,
) -> Result<(), CoreError> {
    let registry = state.task_registry.clone();

    // The durable row is the lifecycle source of truth. Mark it running
    // before provider/tool work begins so boot recovery can safely classify
    // a process crash at any later point.
    let execution = state
        .memory
        .get_subagent_execution(task_id)
        .await?
        .ok_or_else(|| {
            CoreError::Memory(openlet_core::error::MemoryError::Io(
                "subagent execution missing".into(),
            ))
        })?;
    let execution_version = state
        .memory
        .patch_subagent_execution(
            task_id,
            SubagentExecutionPatch {
                expected_version: execution.version,
                status: SubagentExecutionStatus::Running,
                terminal_reason: None,
                output: None,
                cost_usd: None,
            },
        )
        .await?
        .ok_or_else(|| {
            CoreError::Memory(openlet_core::error::MemoryError::Io(
                "subagent execution state conflict".into(),
            ))
        })?
        .version;

    let tools = crate::turn_driver::tool_specs(&state);

    let read_history = state
        .read_histories
        .entry(child_session_id)
        .or_default()
        .clone();

    let agent_def = state.agent_registry.get(&agent_slug).cloned().map(Arc::new);

    let child_meta = state.memory.get_session(child_session_id).await?;
    let child_model = child_meta.as_ref().and_then(|m| m.model.clone());

    // Subagents get their own agent's system prompt too (identity + tool
    // catalog), composed against the child agent's workspace root.
    let system_prompt = crate::turn_driver::compose_agent_system_prompt(
        agent_def.as_ref(),
        &agent_resources.spec.workspace_root,
    );

    let projection_caps = ProjectionCaps::default();
    let handles =
        crate::turn_driver::runtime_handles(&state, agent_resources.fs.clone(), child_perm);
    let llm_messages = openlet_core::runtime::prepare_session_messages(
        &handles,
        child_session_id,
        projection_caps,
        openlet_core::runtime::ReminderRequestContext::default(),
    )
    .await?;
    let loop_ctx = LoopContext {
        agent_id: agent_resources.spec.id,
        handles,
        read_history,
        mode: child_meta.map(|m| m.permission_mode).unwrap_or_default(),
        max_steps: crate::turn_driver::MAX_TURN_STEPS,
        projection_caps,
        agent: agent_def,
    };

    let mut input = crate::turn_driver::build_turn_input(
        child_session_id,
        llm_messages,
        tools,
        child_model.clone(),
        system_prompt.clone(),
    );

    // Recover messages accepted before a process exit (or while this child
    // was interrupted). Acknowledge only after each wrapped part is durable,
    // so an error leaves the message available to a later explicit resume.
    let recovered_inbox = state
        .memory
        .list_pending_subagent_inbox_messages(task_id)
        .await?;
    if !recovered_inbox.is_empty() {
        let mut acknowledged = Vec::new();
        for message in recovered_inbox {
            let wrapped = crate::injected_turn::wrap_untrusted(
                &crate::app_state::TurnOrigin::SiblingMessage { from: message.from },
                &message.body,
            );
            seed_child_message(&state, child_session_id, wrapped).await?;
            acknowledged.push(message.id);
        }
        state
            .memory
            .acknowledge_subagent_inbox_messages(task_id, &acknowledged)
            .await?;
        let messages = openlet_core::runtime::prepare_session_messages(
            &loop_ctx.handles,
            child_session_id,
            ProjectionCaps::default(),
            openlet_core::runtime::ReminderRequestContext::default(),
        )
        .await?;
        input = crate::turn_driver::build_turn_input(
            child_session_id,
            messages,
            crate::turn_driver::tool_specs(&state),
            child_model.clone(),
            system_prompt.clone(),
        );
    }

    let memory = crate::turn_driver::memory_arc(&state);

    // Re-armable driver loop (Phase 2). Each iteration drives ONE objective
    // to completion; after it, `should_rearm` decides whether the task
    // stays alive to receive an external wake (a sibling message inbox —
    // Phase 4 — or a promotion resume — Phase 3) or is genuinely terminal.
    //
    // For a plain sync/background child with no inbox and no promotion,
    // `should_rearm` returns false immediately, so the loop runs exactly
    // once — byte-for-byte the pre-Phase-2 single-shot behavior. The ONLY
    // structural change is that `finalize` (quota release) now happens
    // AFTER the outer loop, so a resumable subagent holds its slot only
    // while genuinely alive.
    let mut final_status;
    loop {
        let outcome = state
            .runtime
            .run_loop(
                &memory,
                loop_ctx.clone(),
                input.clone(),
                child_cancel.clone(),
            )
            .await;

        // Collect the final assistant text into the task output buffer.
        // `final_assistant_message_id` is `Option`: `None` means no model
        // turn produced an assistant message (e.g. a before_turn hook
        // halted turn 0). Skip `list_parts` entirely in that case — the
        // subagent output is correctly empty, NOT the nil-UUID's (empty)
        // part list masquerading as a real lookup.
        if let Ok(o) = &outcome
            && let Some(final_msg_id) = o.final_assistant_message_id
            && let Ok(parts) = state
                .memory
                .list_parts(child_session_id, final_msg_id)
                .await
        {
            let mut buf = String::new();
            for p in parts {
                if let Part::Text { text, .. } = p {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(&text);
                }
            }
            if !buf.is_empty() {
                registry.append_output(task_id, &buf).await;
            }
        }

        // Cost rollup — child cost flows into parent's cumulative ledger.
        let child_cost = state.runtime.session_cost(child_session_id);
        if !child_cost.is_zero() {
            registry.add_cost(task_id, child_cost).await;
            state
                .runtime
                .add_session_cost_external(parent_session_id, child_cost);
        }

        final_status = match &outcome {
            Ok(_) => TaskStatus::Finished,
            Err(_) if child_cancel.is_cancelled() && registry.was_interrupted(task_id) => {
                TaskStatus::Interrupted
            }
            Err(_) if child_cancel.is_cancelled() => TaskStatus::Cancelled,
            Err(e) => TaskStatus::Failed(e.to_string()),
        };

        // Re-arm decision (Phase 4). NON-BLOCKING: a task re-enters
        // `run_loop` only when sibling messages ALREADY arrived while the
        // just-finished objective was running (drained here without
        // parking). A cancelled task never re-arms; a task with an empty
        // inbox breaks immediately and settles — preserving Phase 1
        // single-shot semantics and, critically, NOT parking forever (which
        // would hang a sync parent's `await_completion` and pin the quota
        // slot). A message that lands after the sibling settles is refused
        // at the sender with a typed "not addressable" error rather than
        // silently delivered to a dead task (Finding 2) — an acceptable MVP
        // bound over an unbounded idle-park that leaks resources.
        if child_cancel.is_cancelled() || !registry.inbox_nonempty(task_id) {
            break;
        }
        // Drain queued messages and seed each as an untrusted-wrapped user
        // turn in the child's OWN session, then rebuild the projection so
        // the next `run_loop` sees them.
        let msgs = registry.drain_inbox(task_id);
        let mut acknowledged = Vec::new();
        for m in &msgs {
            let wrapped = crate::injected_turn::wrap_untrusted(
                &crate::app_state::TurnOrigin::SiblingMessage {
                    from: m.from.clone(),
                },
                &m.body,
            );
            if seed_child_message(&state, child_session_id, wrapped)
                .await
                .is_err()
            {
                break;
            }
            if let Some(id) = &m.id {
                acknowledged.push(id.clone());
            }
        }
        if !acknowledged.is_empty()
            && state
                .memory
                .acknowledge_subagent_inbox_messages(task_id, &acknowledged)
                .await
                .is_err()
        {
            break;
        }
        // Re-project the child session so the drained messages enter the
        // next objective's input.
        match openlet_core::runtime::prepare_session_messages(
            &loop_ctx.handles,
            child_session_id,
            ProjectionCaps::default(),
            openlet_core::runtime::ReminderRequestContext::default(),
        )
        .await
        {
            Ok(msgs) => {
                input = crate::turn_driver::build_turn_input(
                    child_session_id,
                    msgs,
                    crate::turn_driver::tool_specs(&state),
                    child_model.clone(),
                    system_prompt.clone(),
                );
            }
            Err(_) => break,
        }
    }

    // Terminal side-effect (ONCE, after the loop). Foreground callers own
    // their original tool result; background callers own exactly one typed
    // reminder/outbox notification. Lifecycle SSE never carries child output.
    let snap = registry
        .poll_async(task_id)
        .await
        .map(|s| s.output)
        .unwrap_or_default();
    let total_cost = state.runtime.session_cost(child_session_id);
    let cost_str = if total_cost.is_zero() {
        None
    } else {
        Some(format!("{total_cost:.4}"))
    };
    let execution_status = match &final_status {
        TaskStatus::Finished => SubagentExecutionStatus::Finished,
        TaskStatus::Cancelled => SubagentExecutionStatus::Cancelled,
        TaskStatus::Interrupted => SubagentExecutionStatus::Interrupted,
        TaskStatus::Failed(_) => SubagentExecutionStatus::Failed,
        TaskStatus::Running => SubagentExecutionStatus::Interrupted,
    };
    let terminal_reason = match &final_status {
        TaskStatus::Failed(error) => Some(error.clone()),
        TaskStatus::Cancelled => Some("cancelled".to_string()),
        TaskStatus::Interrupted => Some("interrupted".to_string()),
        _ => None,
    };
    if state
        .memory
        .patch_subagent_execution(
            task_id,
            SubagentExecutionPatch {
                expected_version: execution_version,
                status: execution_status,
                terminal_reason,
                output: Some(snap.clone()),
                cost_usd: cost_str.clone(),
            },
        )
        .await?
        .is_none()
    {
        tracing::warn!(%task_id, execution_version, "subagent terminal execution state conflicted");
    }
    // Guard the terminal side-effect behind the one-shot `settled` slot so
    // a task cannot both inject AND publish-with-output. Claiming BEFORE
    // `finalize` (which removes the handle) ensures the slot is present.
    // The quota decrement in `finalize` remains ungated (`saturating_dec`).
    let delivery = registry
        .settle_delivery(task_id)
        .unwrap_or(DeliveryOwnership::TerminalForeground);
    registry.set_status(task_id, final_status.clone()).await;
    if registry.claim_settle(task_id) {
        if delivery == DeliveryOwnership::TerminalBackground {
            // Persist the terminal notification before scheduling the parent
            // wake. The SQLite delivery key makes retries/reconnects
            // idempotent and keeps output out of public lifecycle frames.
            let reminder_persisted = match state
                .memory
                .append_background_task_settled(
                    openlet_core::adapters::memory_store::BackgroundTaskSettlement {
                        parent_session_id,
                        task_id: task_id.0.to_string(),
                        child_session_id,
                        status: final_status.label().to_string(),
                        output: snap.clone(),
                        cost_usd: cost_str.clone(),
                    },
                )
                .await
            {
                Ok(_) => true,
                Err(error) => {
                    tracing::error!(
                        %task_id,
                        %parent_session_id,
                        error = %error,
                        "background task settlement reminder was not persisted; parent wake deferred"
                    );
                    false
                }
            };
            if reminder_persisted
                && let Err(error) = crate::injected_turn::enqueue_background_task_delivery(
                    &state,
                    parent_session_id,
                    &task_id.0.to_string(),
                )
                .await
            {
                tracing::error!(%task_id, %parent_session_id, error = %error, "background settlement remains pending for recovery");
            }
        }
        let _ = state
            .events
            .publish(
                AgentEvent::SubagentSettled {
                    task_id: task_id.0,
                    child_session_id,
                    parent_session_id,
                    status: final_status.label().to_string(),
                    cost_usd: cost_str,
                },
                Persistence::Durable,
            )
            .await;
    }

    // Remove from the sibling roster — the task is no longer
    // background-alive, so it must not be addressable by a `send_message`
    // (Finding 2: no silent misroute to a finalized sibling). Then emit a
    // roster-change frame so the TUI @mention typeahead drops the entry.
    registry.remove_from_roster(root_session_id, &handle_name);
    publish_roster(&state, root_session_id).await;

    registry.finalize(task_id);
    Ok(())
}

/// Emit the current `subagent.roster` snapshot for `root` so SSE consumers
/// (the TUI @mention typeahead) track the live named siblings. Called on
/// any roster change (spawn registers via the spawner; settle removes here).
pub(crate) async fn publish_roster(state: &AppState, root: SessionId) {
    let entries = state
        .task_registry
        .roster_snapshot(root)
        .into_iter()
        .map(
            |(name, task_id, generation)| openlet_core::types::event::RosterFrameEntry {
                name: name.to_string(),
                task_id: task_id.0,
                generation,
            },
        )
        .collect();
    let _ = state
        .events
        .publish(
            AgentEvent::SubagentRoster {
                root_session_id: root,
                entries,
            },
            Persistence::Durable,
        )
        .await;
}

/// Seed a sibling message as a single-text-part user message in the CHILD's
/// own session so the next `run_loop` projection includes it. The body is
/// already untrusted-wrapped by the caller.
async fn seed_child_message(
    state: &AppState,
    child_session_id: SessionId,
    wrapped_body: String,
) -> Result<(), CoreError> {
    use openlet_core::types::message::{Message, MessageId, Role};
    let msg = Message {
        id: MessageId::new(),
        session_id: child_session_id,
        role: Role::User,
        created_at: chrono::Utc::now(),
    };
    let msg_id = state.memory.append_message(child_session_id, msg).await?;
    let part = Part::Text {
        id: openlet_core::types::part::PartId::new(),
        text: wrapped_body,
    };
    state.memory.append_part(msg_id, part).await?;
    Ok(())
}
