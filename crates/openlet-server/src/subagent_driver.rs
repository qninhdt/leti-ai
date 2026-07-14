//! Subagent driver task.
//!
//! Runs a nested `ConversationRuntime::run_loop` on a child session,
//! mirrors output + cost into the task registry, and emits the terminal
//! `SubagentSettled` event. Extracted from `subagent_spawner.rs` to keep
//! the spawner focused on admission + task lifecycle.

use std::sync::Arc;

use openlet_core::adapters::event_sink::Persistence;
use openlet_core::error::CoreError;
use openlet_core::projection::ProjectionCaps;
use openlet_core::runtime::LoopContext;
use openlet_core::runtime::subagent::{TaskId, TaskStatus};
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

    let llm_messages =
        crate::turn_driver::project_session(&state, child_session_id, ProjectionCaps::default())
            .await?;

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

    let loop_ctx = LoopContext {
        agent_id: agent_resources.spec.id,
        handles: crate::turn_driver::runtime_handles(
            &state,
            agent_resources.fs.clone(),
            child_perm,
        ),
        read_history,
        mode: child_meta.map(|m| m.permission_mode).unwrap_or_default(),
        max_steps: crate::turn_driver::MAX_TURN_STEPS,
        agent: agent_def,
    };

    let mut input = crate::turn_driver::build_turn_input(
        child_session_id,
        llm_messages,
        tools,
        child_model.clone(),
        system_prompt.clone(),
    );

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
        }
        // Re-project the child session so the drained messages enter the
        // next objective's input.
        match crate::turn_driver::project_session(
            &state,
            child_session_id,
            ProjectionCaps::default(),
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

    // Terminal side-effect (ONCE, after the loop). Promotion changes HOW
    // the output is delivered (Validation Session 1, OpenCode synthetic-
    // message pattern):
    //   - PROMOTED task → the output re-enters the parent conversation as
    //     an injected `InjectedResult` turn (untrusted-wrapped, fail-closed
    //     Ask via the Phase 2 queue). `SubagentSettled` then carries status
    //     + cost ONLY (empty output) so the result is not double-rendered.
    //   - NON-promoted task → `SubagentSettled` carries the output as
    //     before; the parent polls via `task_status`.
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
    let was_promoted = registry.is_promoted(task_id);
    registry.set_status(task_id, final_status.clone()).await;

    // Guard the terminal side-effect behind the one-shot `settled` slot so
    // a task cannot both inject AND publish-with-output. Claiming BEFORE
    // `finalize` (which removes the handle) ensures the slot is present.
    // The quota decrement in `finalize` remains ungated (`saturating_dec`).
    if registry.claim_settle(task_id) {
        // A promoted task delivers its result via injection; a non-promoted
        // one carries it in the settled frame. Only inject a genuinely
        // finished promoted task's output — a cancelled/failed promoted
        // task just settles (no injected result turn to render).
        let injected =
            was_promoted && matches!(final_status, TaskStatus::Finished) && !snap.is_empty();
        if injected {
            crate::injected_turn::enqueue_or_start_turn(
                &state,
                parent_session_id,
                snap.clone(),
                crate::app_state::TurnOrigin::InjectedResult { task_id },
            );
        }
        let settled_output = if injected { String::new() } else { snap };
        let _ = state
            .events
            .publish(
                AgentEvent::SubagentSettled {
                    task_id: task_id.0,
                    parent_session_id,
                    output: settled_output,
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
