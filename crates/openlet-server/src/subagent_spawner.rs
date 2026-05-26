//! Server-side `SubagentSpawner` impl.
//!
//! Bridges the in-process subagent toolset to a real
//! `ConversationRuntime::run_loop` driven by a tokio task. The spawner
//! is constructed BEFORE `AppState` (so `core-tools` can register
//! `subagent_task` with a handle), then late-bound via [`set_state`]
//! once `AppState` is built.
//!
//! Cost rollup (F4.4): every turn the child runtime bills is added
//! both to the child task's `cost_usd` and to the PARENT session's
//! cumulative cost via `ConversationRuntime::add_session_cost_external`.
//! That keeps the parent's `session_cost` query consistent with the
//! true tree-wide spend.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::error::CoreError;
use openlet_core::projection::{ProjectionCaps, project_for_llm};
use openlet_core::runtime::subagent::{SpawnError, TaskId, TaskStatus, plan_subagent_spawn};
use openlet_core::runtime::{LoopContext, TurnInput};
use openlet_core::tools::builtins::subagent_task::SubagentSpawner;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::message::{Message, MessageId, Role};
use openlet_core::types::part::Part;
use openlet_core::types::session::SessionId;
use rust_decimal::Decimal;
use tokio::sync::OnceCell;

use crate::app_state::AppState;

/// Late-bound spawner. `Arc` so the same instance can be cloned into
/// `CoreToolsPlugin` and into `AppState` for cancel-cascade hooks.
pub struct RuntimeSubagentSpawner {
    state: Arc<OnceCell<AppState>>,
    max_depth: u8,
}

impl Default for RuntimeSubagentSpawner {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeSubagentSpawner {
    /// Construct an unbound spawner. Caller MUST call [`set_state`]
    /// exactly once after `AppState` is built, before any tool dispatch
    /// reaches `subagent_task`.
    #[must_use]
    pub fn new() -> Self {
        let max_depth = std::env::var("OPENLET_SUBAGENT_MAX_DEPTH")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(openlet_core::runtime::subagent::DEFAULT_MAX_DEPTH);
        Self {
            state: Arc::new(OnceCell::new()),
            max_depth,
        }
    }

    /// Bind the live `AppState`. Idempotent: subsequent calls are
    /// silently ignored — boot wiring sets this exactly once.
    pub fn set_state(&self, state: AppState) {
        let _ = self.state.set(state);
    }

    fn state(&self) -> Result<&AppState, SpawnError> {
        self.state
            .get()
            .ok_or_else(|| SpawnError::Internal("subagent spawner not bound to AppState".into()))
    }

    /// Resolve the root session id by walking parent_session_id up to a
    /// session with `parent_session_id = None`. Caps at depth+1 walks
    /// to keep the lookup bounded even on a corrupt chain.
    async fn root_session_of(&self, sid: SessionId, fallback: SessionId) -> SessionId {
        let Ok(state) = self.state() else {
            return fallback;
        };
        let mut current = sid;
        for _ in 0..(self.max_depth as usize + 2) {
            match state.memory.get_session(current).await {
                Ok(Some(meta)) => match meta.parent_session_id {
                    Some(p) => current = p,
                    None => return current,
                },
                _ => return current,
            }
        }
        current
    }
}

#[async_trait]
impl SubagentSpawner for RuntimeSubagentSpawner {
    async fn spawn(
        &self,
        ctx: &ToolCtx,
        subagent_type: &str,
        objective: &str,
    ) -> Result<TaskId, SpawnError> {
        let state = self.state()?;
        let parent_meta = state
            .memory
            .get_session(ctx.session_id)
            .await
            .map_err(|e| SpawnError::Internal(format!("memory: {e}")))?
            .ok_or_else(|| SpawnError::Internal("parent session missing".into()))?;
        let root = self.root_session_of(ctx.session_id, ctx.session_id).await;

        let plan = plan_subagent_spawn(
            &parent_meta,
            subagent_type,
            &state.agent_registry,
            ctx.permission.clone(),
            &ctx.cancel,
            &state.task_registry,
            root,
            self.max_depth,
        )?;

        // Persist the child session synchronously so SSE consumers see
        // the row before SubagentStarted fires. Fall back to the parent
        // agent_id since the API doesn't expose a per-slug AgentId yet.
        if let Err(e) = state
            .memory
            .create_session(parent_meta.agent_id, Some(parent_meta.id))
            .await
        {
            state.task_registry.finalize(plan.task_id);
            return Err(SpawnError::Internal(format!("create child session: {e}")));
        }

        // Seed the child with a single user message holding the objective.
        let user_msg = Message {
            id: MessageId::new(),
            session_id: plan.child.id,
            role: Role::User,
            created_at: Utc::now(),
        };
        let user_msg_id = state
            .memory
            .append_message(plan.child.id, user_msg)
            .await
            .map_err(|e| SpawnError::Internal(format!("seed user message: {e}")))?;
        let part = Part::Text {
            id: openlet_core::types::part::PartId::new(),
            text: objective.to_string(),
        };
        let _ = state.memory.append_part(user_msg_id, part).await;

        let _ = state
            .events
            .publish(
                AgentEvent::SubagentStarted {
                    task_id: plan.task_id.0,
                    parent_session_id: parent_meta.id,
                    subagent_type: subagent_type.to_string(),
                },
                Persistence::Durable,
            )
            .await;

        let driver_state = state.clone();
        let task_id = plan.task_id;
        let child_perm = plan.child_perm.clone();
        let child_cancel = plan.child_cancel.clone();
        let agent_slug = plan.agent_slug.clone();
        let parent_id = parent_meta.id;
        let agent_resources = state
            .agents
            .get(&parent_meta.agent_id)
            .cloned()
            .ok_or_else(|| {
                state.task_registry.finalize(plan.task_id);
                SpawnError::Internal("parent agent_resources missing".into())
            })?;
        let child_session_id = plan.child.id;

        tokio::spawn(async move {
            let _ = drive_subagent(
                driver_state,
                task_id,
                parent_id,
                child_session_id,
                agent_slug,
                child_perm,
                child_cancel,
                agent_resources,
            )
            .await;
        });

        Ok(plan.task_id)
    }

    async fn await_completion(
        &self,
        task_id: TaskId,
    ) -> Result<(String, Option<String>, TaskStatus), SpawnError> {
        let state = self.state()?;
        let snap =
            state
                .task_registry
                .await_completion(task_id)
                .await
                .ok_or(SpawnError::Internal(
                    "task vanished before completion".into(),
                ))?;
        let cost = if snap.cost_usd.is_zero() {
            None
        } else {
            Some(format!("{:.4}", snap.cost_usd))
        };
        Ok((snap.output, cost, snap.status))
    }
}

/// Driver task — runs a nested `ConversationRuntime::run_loop` on the
/// child session, mirrors output + cost into the task registry, and
/// emits the terminal `SubagentFinished` event.
#[allow(clippy::too_many_arguments)]
async fn drive_subagent(
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

    let messages = state.memory.list_messages(child_session_id).await?;
    let mut parts_by_msg: HashMap<MessageId, Vec<Part>> = HashMap::with_capacity(messages.len());
    for m in &messages {
        let parts = state.memory.list_parts(child_session_id, m.id).await?;
        parts_by_msg.insert(m.id, parts);
    }
    let llm_messages = project_for_llm(&messages, &parts_by_msg, ProjectionCaps::default());

    let tools: Vec<openlet_core::adapters::model_provider::ToolSpec> = state
        .tool_registry
        .iter()
        .map(
            |(name, handle)| openlet_core::adapters::model_provider::ToolSpec {
                name: name.to_string(),
                description: handle.description().to_string(),
                parameters: handle.input_schema(),
            },
        )
        .collect();

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
        max_steps: 50,
        agent: agent_def,
        hook_chains: state.hook_chains.clone(),
        questions: state.questions.clone(),
        memory: state.memory.clone(),
        task_registry: state.task_registry.clone(),
        agent_registry: state.agent_registry.clone(),
    };

    let input = TurnInput {
        session_id: child_session_id,
        messages: llm_messages,
        system_prompt: None,
        model: None,
        max_tokens: None,
        temperature: None,
        tools,
    };

    let memory: Arc<dyn openlet_core::adapters::MemoryStore> = state.memory.clone();
    let outcome = state
        .runtime
        .run_loop(&memory, loop_ctx, input, child_cancel.clone())
        .await;

    // Collect the final assistant text into the task output buffer.
    if let Ok(o) = &outcome {
        if let Ok(parts) = state
            .memory
            .list_parts(child_session_id, o.final_assistant_message_id)
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
                output: snap,
                cost_usd: cost_str,
            },
            Persistence::Durable,
        )
        .await;

    registry.finalize(task_id);
    let _ = Decimal::ZERO; // silence unused import on minimal builds
    Ok(())
}
