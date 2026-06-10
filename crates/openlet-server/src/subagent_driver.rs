//! Subagent driver task.
//!
//! Runs a nested `ConversationRuntime::run_loop` on a child session,
//! mirrors output + cost into the task registry, and emits the terminal
//! `SubagentFinished` event. Extracted from `subagent_spawner.rs` to keep
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
/// emits the terminal `SubagentFinished` event.
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

    let loop_ctx = LoopContext {
        agent_id: agent_resources.spec.id,
        fs: agent_resources.fs.clone(),
        permission: child_perm,
        events: state.events.clone(),
        artifacts: state.artifacts.clone(),
        registry: state.tool_registry.clone(),
        read_history,
        mode: state
            .memory
            .get_session(child_session_id)
            .await?
            .map(|m| m.permission_mode)
            .unwrap_or_default(),
        max_steps: crate::turn_driver::MAX_TURN_STEPS,
        agent: agent_def,
        hook_chains: state.hook_chains.clone(),
        questions: state.questions.clone(),
        memory: state.memory.clone(),
        task_registry: state.task_registry.clone(),
        agent_registry: state.agent_registry.clone(),
    };

    let input = crate::turn_driver::build_turn_input(child_session_id, llm_messages, tools);

    let memory = crate::turn_driver::memory_arc(&state);
    let outcome = state
        .runtime
        .run_loop(&memory, loop_ctx, input, child_cancel.clone())
        .await;

    // Collect the final assistant text into the task output buffer.
    // `final_assistant_message_id` is `Option`: `None` means no model turn
    // produced an assistant message (e.g. a before_turn hook halted turn
    // 0). Skip `list_parts` entirely in that case — the subagent output is
    // correctly empty, NOT the nil-UUID's (empty) part list masquerading
    // as a real lookup.
    if let Ok(o) = &outcome {
        if let Some(final_msg_id) = o.final_assistant_message_id {
            if let Ok(parts) = state
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

    let final_status = match &outcome {
        Ok(_) => TaskStatus::Finished,
        Err(_) if child_cancel.is_cancelled() => TaskStatus::Cancelled,
        Err(e) => TaskStatus::Failed(e.to_string()),
    };
    registry.set_status(task_id, final_status.clone()).await;

    let snap = registry
        .poll_async(task_id)
        .await
        .map(|s| s.output)
        .unwrap_or_default();
    let cost_str = if child_cost.is_zero() {
        None
    } else {
        Some(format!("{child_cost:.4}"))
    };
    let _ = state
        .events
        .publish(
            AgentEvent::SubagentFinished {
                task_id: task_id.0,
                parent_session_id,
                output: snap,
                cost_usd: cost_str,
            },
            Persistence::Durable,
        )
        .await;

    registry.finalize(task_id);
    Ok(())
}
