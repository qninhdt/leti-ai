//! Server-side `SubagentSpawner` impl.
//!
//! Bridges the in-process subagent toolset to a real
//! `ConversationRuntime::run_loop` driven by a tokio task. The spawner
//! is constructed BEFORE `AppState` (so `core-tools` can register
//! `subagent_task` with a handle), then late-bound via [`set_state`]
//! once `AppState` is built.
//!
//! Cost rollup: every turn the child runtime bills is added
//! both to the child task's `cost_usd` and to the PARENT session's
//! cumulative cost via `ConversationRuntime::add_session_cost_external`.
//! That keeps the parent's `session_cost` query consistent with the
//! true tree-wide spend.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use futures::FutureExt;
use leti_core::adapters::event_sink::Persistence;
use leti_core::adapters::tool_executor::ToolCtx;
use leti_core::runtime::subagent::{
    BackgroundTransition, DeliveryOwnership, SpawnError, SubagentExecution,
    SubagentExecutionStatus, TaskId, TaskStatus, plan_subagent_continuation, plan_subagent_spawn,
};
use leti_core::tools::builtins::subagent_task::{SpawnedSubagent, SubagentSpawner};
use leti_core::types::event::AgentEvent;
use leti_core::types::message::{Message, MessageId, Role};
use leti_core::types::part::Part;
use leti_core::types::session::SessionId;
use tokio::sync::OnceCell;

use crate::app_state::AppState;
use crate::subagent_driver::publish_roster;

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
        let max_depth = std::env::var("LETI_SUBAGENT_MAX_DEPTH")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(leti_core::runtime::subagent::DEFAULT_MAX_DEPTH);
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
    /// session with `parent_session_id = None`. Caps at depth+2 walks to
    /// keep the lookup bounded even on a corrupt chain.
    ///
    /// A DB error from `get_session` is PROPAGATED, not swallowed.
    /// Silently returning `current` on a transient memory error would
    /// resolve the WRONG root and admit the subagent against the wrong
    /// quota bucket (a quota-bypass / accounting-corruption vector). A
    /// genuine "session not found" (`Ok(None)`) is still treated as a
    /// terminal root — the chain simply ends there.
    async fn root_session_of(&self, sid: SessionId) -> Result<SessionId, SpawnError> {
        let state = self.state()?;
        let mut current = sid;
        for _ in 0..(self.max_depth as usize + 2) {
            match state.memory.get_session(current).await {
                Ok(Some(meta)) => match meta.parent_session_id {
                    Some(p) => current = p,
                    None => return Ok(current),
                },
                // Session row absent — treat `current` as the terminal root.
                Ok(None) => return Ok(current),
                // Transient/store error — fail fast instead of resolving a
                // wrong root and corrupting quota accounting.
                Err(e) => {
                    return Err(SpawnError::Internal(format!("root resolution failed: {e}")));
                }
            }
        }
        // Depth cap reached (corrupt/cyclic parent chain) — bounded fallback.
        Ok(current)
    }

    async fn durable_completion(
        &self,
        task_id: TaskId,
    ) -> Result<(String, Option<String>, TaskStatus), SpawnError> {
        let state = self.state()?;
        let execution = state
            .memory
            .get_subagent_execution(task_id)
            .await
            .map_err(|e| SpawnError::Internal(format!("load task execution: {e}")))?
            .ok_or_else(|| SpawnError::Internal("task vanished before completion".into()))?;
        let status = match execution.status {
            SubagentExecutionStatus::Finished => TaskStatus::Finished,
            SubagentExecutionStatus::Cancelled => TaskStatus::Cancelled,
            SubagentExecutionStatus::Interrupted => TaskStatus::Interrupted,
            SubagentExecutionStatus::Failed => TaskStatus::Failed(
                execution
                    .terminal_reason
                    .unwrap_or_else(|| "task failed".into()),
            ),
            SubagentExecutionStatus::Pending | SubagentExecutionStatus::Running => {
                return Err(SpawnError::Internal(
                    "task is still running but has no live handle".into(),
                ));
            }
        };
        Ok((execution.output, execution.cost_usd, status))
    }
}

#[async_trait]
impl SubagentSpawner for RuntimeSubagentSpawner {
    async fn spawn(
        &self,
        ctx: &ToolCtx,
        subagent_type: &str,
        objective: &str,
        scope: Option<&str>,
        background: bool,
    ) -> Result<SpawnedSubagent, SpawnError> {
        let state = self.state()?;
        let parent_meta = state
            .memory
            .get_session(ctx.session_id)
            .await
            .map_err(|e| SpawnError::Internal(format!("memory: {e}")))?
            .ok_or_else(|| SpawnError::Internal("parent session missing".into()))?;
        let root = self.root_session_of(ctx.session_id).await?;

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

        // A task born in background mode uses the exact same ownership CAS as
        // a later TUI conversion. Establish it before any driver can settle.
        if background
            && !matches!(
                state
                    .task_registry
                    .background_task(plan.task_id, parent_meta.id),
                BackgroundTransition::Backgrounded | BackgroundTransition::AlreadyBackground
            )
        {
            state.task_registry.finalize(plan.task_id);
            return Err(SpawnError::Internal(
                "background task ownership could not be initialized".into(),
            ));
        }

        // Persist child identity and execution together. This prevents a
        // restart from observing an addressable child session with no task
        // lifecycle record (or vice versa).
        let now = Utc::now();
        let execution = SubagentExecution {
            task_id: plan.task_id,
            root_session_id: root,
            parent_session_id: parent_meta.id,
            child_session_id: plan.child.id,
            agent_slug: plan.agent_slug.as_str().to_string(),
            objective: objective.to_string(),
            scope: scope.map(str::to_string),
            background,
            status: SubagentExecutionStatus::Pending,
            terminal_reason: None,
            output: String::new(),
            cost_usd: None,
            created_at: now,
            updated_at: now,
            finished_at: None,
            version: 0,
        };
        if let Err(e) = state
            .memory
            .create_subagent_session_and_execution(plan.child.clone(), execution)
            .await
        {
            state.task_registry.finalize(plan.task_id);
            return Err(SpawnError::Internal(format!("create child session: {e}")));
        }

        let agent_resources = match state.agents.get(&parent_meta.agent_id).cloned() {
            Some(resources) => resources,
            None => {
                state.task_registry.finalize(plan.task_id);
                return Err(SpawnError::Internal(
                    "parent agent_resources missing".into(),
                ));
            }
        };

        // Seed the child with a single user message holding the objective.
        let user_msg = Message {
            id: MessageId::new(),
            session_id: plan.child.id,
            role: Role::User,
            created_at: Utc::now(),
        };
        let user_msg_id = match state.memory.append_message(plan.child.id, user_msg).await {
            Ok(id) => id,
            Err(e) => {
                state.task_registry.finalize(plan.task_id);
                return Err(SpawnError::Internal(format!("seed user message: {e}")));
            }
        };
        let part = Part::Text {
            id: leti_core::types::part::PartId::new(),
            text: objective.to_string(),
        };
        // The seed user message must carry the objective. Silently
        // dropping it on memory failure leaves the LLM staring at an
        // empty user turn and producing garbage. Surface as
        // `SpawnError::Internal` so the caller fails fast and the
        // operator sees the storage error instead of a confused agent.
        if let Err(e) = state.memory.append_part(user_msg_id, part).await {
            state.task_registry.finalize(plan.task_id);
            return Err(SpawnError::Internal(format!("seed user part: {e}")));
        }

        let _ = state
            .events
            .publish(
                AgentEvent::SubagentSpawned {
                    task_id: plan.task_id.0,
                    tool_call_id: ctx.call_id.clone(),
                    child_session_id: plan.child.id,
                    parent_session_id: parent_meta.id,
                    subagent_type: subagent_type.to_string(),
                    objective: objective.to_string(),
                    description: scope.map(str::to_string),
                    background,
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
        let child_session_id = plan.child.id;
        let handle_name = plan.handle_name.clone();
        let driver_root = root;
        let parent_ext = ctx.ext.clone();

        // Emit a roster-change frame so the TUI @mention typeahead sees the
        // newly-registered sibling (Phase 4 Finding 11). Best-effort.
        publish_roster(state, driver_root).await;

        tokio::spawn(async move {
            let failure =
                match std::panic::AssertUnwindSafe(crate::subagent_driver::drive_subagent(
                    driver_state.clone(),
                    task_id,
                    parent_id,
                    child_session_id,
                    agent_slug,
                    child_perm,
                    child_cancel,
                    agent_resources,
                    driver_root,
                    handle_name,
                    parent_ext,
                ))
                .catch_unwind()
                .await
                {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error.to_string()),
                    Err(_) => Some("subagent driver panicked".to_string()),
                };
            if let Some(error) = failure {
                // Every admitted task must settle even if setup fails before
                // the driver's normal terminal path. This fallback owns only
                // the exceptional path; a normally settled driver returns
                // Ok and has already published/finalized itself.
                let status = TaskStatus::Failed(error.clone());
                if let Ok(Some(execution)) =
                    driver_state.memory.get_subagent_execution(task_id).await
                {
                    let _ = driver_state
                        .memory
                        .patch_subagent_execution(
                            task_id,
                            leti_core::adapters::memory_store::SubagentExecutionPatch {
                                expected_version: execution.version,
                                status: SubagentExecutionStatus::Failed,
                                terminal_reason: Some(error.clone()),
                                output: Some(error.clone()),
                                cost_usd: None,
                            },
                        )
                        .await;
                }
                let delivery = driver_state
                    .task_registry
                    .settle_delivery(task_id)
                    .unwrap_or(DeliveryOwnership::TerminalForeground);
                driver_state
                    .task_registry
                    .set_status(task_id, status.clone())
                    .await;
                if driver_state.task_registry.claim_settle(task_id) {
                    if delivery == DeliveryOwnership::TerminalBackground {
                        match driver_state
                            .memory
                            .append_background_task_settled(
                                leti_core::adapters::memory_store::BackgroundTaskSettlement {
                                    parent_session_id: parent_id,
                                    task_id: task_id.0.to_string(),
                                    child_session_id,
                                    status: status.label().to_string(),
                                    output: error.clone(),
                                    cost_usd: None,
                                },
                            )
                            .await
                        {
                            Ok(_) => {
                                if let Err(delivery_error) =
                                    crate::injected_turn::enqueue_background_task_delivery(
                                        &driver_state,
                                        parent_id,
                                        &task_id.0.to_string(),
                                    )
                                    .await
                                {
                                    tracing::error!(%task_id, %parent_id, error = %delivery_error, "failed background task settlement remains pending for recovery");
                                }
                            }
                            Err(persist_error) => {
                                tracing::error!(%task_id, %parent_id, error = %persist_error, "failed to persist background task setup failure");
                            }
                        }
                    }
                    let _ = driver_state
                        .events
                        .publish(
                            AgentEvent::SubagentSettled {
                                task_id: task_id.0,
                                child_session_id,
                                parent_session_id: parent_id,
                                status: status.label().to_string(),
                                cost_usd: None,
                            },
                            Persistence::Durable,
                        )
                        .await;
                }
                driver_state.task_registry.finalize(task_id);
                publish_roster(&driver_state, driver_root).await;
            }
        });

        Ok(SpawnedSubagent {
            task_id: plan.task_id,
            child_session_id,
        })
    }

    async fn await_completion(
        &self,
        task_id: TaskId,
    ) -> Result<(String, Option<String>, TaskStatus), SpawnError> {
        let state = self.state()?;
        let snap = state.task_registry.await_completion(task_id).await;
        let Some(snap) = snap else {
            return self.durable_completion(task_id).await;
        };
        let cost = if snap.cost_usd.is_zero() {
            None
        } else {
            Some(format!("{:.4}", snap.cost_usd))
        };
        Ok((snap.output, cost, snap.status))
    }

    async fn await_foreground_completion(
        &self,
        task_id: TaskId,
    ) -> Result<(String, Option<String>, TaskStatus), SpawnError> {
        let state = self.state()?;
        let snap = state
            .task_registry
            .await_foreground_completion(task_id)
            .await;
        let Some(snap) = snap else {
            return self.durable_completion(task_id).await;
        };
        let cost = if snap.cost_usd.is_zero() {
            None
        } else {
            Some(format!("{:.4}", snap.cost_usd))
        };
        Ok((snap.output, cost, snap.status))
    }

    async fn continue_subagent(
        &self,
        ctx: &ToolCtx,
        child_session_id: SessionId,
        objective: &str,
        background: bool,
    ) -> Result<SpawnedSubagent, SpawnError> {
        let state = self.state()?;
        let child = state
            .memory
            .get_session(child_session_id)
            .await
            .map_err(|e| SpawnError::Internal(format!("load child session: {e}")))?
            .ok_or_else(|| SpawnError::Internal("child session missing".into()))?;
        let root = self.root_session_of(ctx.session_id).await?;
        if self.root_session_of(child_session_id).await? != root {
            return Err(SpawnError::Internal(
                "child session belongs to another root".into(),
            ));
        }
        if state
            .memory
            .list_subagent_executions(root, false)
            .await
            .map_err(|e| SpawnError::Internal(format!("list live subagents: {e}")))?
            .iter()
            .any(|execution| execution.child_session_id == child_session_id)
        {
            return Err(SpawnError::Internal(
                "child session already has a live execution".into(),
            ));
        }
        let slug = child
            .current_agent_slug
            .clone()
            .unwrap_or_else(|| "general".into());
        let plan = plan_subagent_continuation(
            &child,
            &slug,
            &state.agent_registry,
            ctx.permission.clone(),
            &ctx.cancel,
            &state.task_registry,
            root,
            self.max_depth,
        )?;
        if background
            && !matches!(
                state
                    .task_registry
                    .background_task(plan.task_id, child.parent_session_id.unwrap_or(root)),
                BackgroundTransition::Backgrounded | BackgroundTransition::AlreadyBackground
            )
        {
            state.task_registry.finalize(plan.task_id);
            return Err(SpawnError::Internal(
                "background task ownership could not be initialized".into(),
            ));
        }
        let now = Utc::now();
        let execution = SubagentExecution {
            task_id: plan.task_id,
            root_session_id: root,
            parent_session_id: child.parent_session_id.unwrap_or(root),
            child_session_id,
            agent_slug: slug.clone(),
            objective: objective.to_string(),
            scope: None,
            background,
            status: SubagentExecutionStatus::Pending,
            terminal_reason: None,
            output: String::new(),
            cost_usd: None,
            created_at: now,
            updated_at: now,
            finished_at: None,
            version: 0,
        };
        if let Err(error) = state.memory.create_subagent_execution(execution).await {
            state.task_registry.finalize(plan.task_id);
            return Err(SpawnError::Internal(format!(
                "persist continued execution: {error}"
            )));
        }
        let message = Message {
            id: MessageId::new(),
            session_id: child_session_id,
            role: Role::User,
            created_at: now,
        };
        let message_id = match state.memory.append_message(child_session_id, message).await {
            Ok(id) => id,
            Err(error) => {
                state.task_registry.finalize(plan.task_id);
                return Err(SpawnError::Internal(format!(
                    "seed continuation message: {error}"
                )));
            }
        };
        if let Err(error) = state
            .memory
            .append_part(
                message_id,
                Part::Text {
                    id: leti_core::types::part::PartId::new(),
                    text: objective.to_string(),
                },
            )
            .await
        {
            state.task_registry.finalize(plan.task_id);
            return Err(SpawnError::Internal(format!(
                "seed continuation part: {error}"
            )));
        }
        let resources = state.agents.get(&child.agent_id).cloned().ok_or_else(|| {
            state.task_registry.finalize(plan.task_id);
            SpawnError::Internal("child agent resources missing".into())
        })?;
        let _ = state
            .events
            .publish(
                AgentEvent::SubagentSpawned {
                    task_id: plan.task_id.0,
                    tool_call_id: ctx.call_id.clone(),
                    child_session_id,
                    parent_session_id: child.parent_session_id.unwrap_or(root),
                    subagent_type: slug,
                    objective: objective.to_string(),
                    description: Some("continuation".into()),
                    background,
                },
                Persistence::Durable,
            )
            .await;
        let driver_state = state.clone();
        let task_id = plan.task_id;
        let parent_id = child.parent_session_id.unwrap_or(root);
        let handle_name = plan.handle_name.clone();
        let agent_slug = plan.agent_slug.clone();
        let child_perm = plan.child_perm.clone();
        let parent_ext = ctx.ext.clone();
        let child_cancel = plan.child_cancel.clone();
        publish_roster(state, root).await;
        tokio::spawn(async move {
            let result = std::panic::AssertUnwindSafe(crate::subagent_driver::drive_subagent(
                driver_state.clone(),
                task_id,
                parent_id,
                child_session_id,
                agent_slug,
                child_perm,
                child_cancel,
                resources,
                root,
                handle_name,
                parent_ext,
            ))
            .catch_unwind()
            .await;
            if !matches!(result, Ok(Ok(()))) {
                let reason = match result {
                    Ok(Err(error)) => error.to_string(),
                    Err(_) => "subagent continuation driver panicked".into(),
                    Ok(Ok(())) => unreachable!(),
                };
                if let Ok(Some(execution)) =
                    driver_state.memory.get_subagent_execution(task_id).await
                {
                    let _ = driver_state
                        .memory
                        .patch_subagent_execution(
                            task_id,
                            leti_core::adapters::memory_store::SubagentExecutionPatch {
                                expected_version: execution.version,
                                status: SubagentExecutionStatus::Failed,
                                terminal_reason: Some(reason.clone()),
                                output: Some(reason),
                                cost_usd: None,
                            },
                        )
                        .await;
                }
                driver_state
                    .task_registry
                    .set_status(
                        task_id,
                        TaskStatus::Failed("continuation driver failed".into()),
                    )
                    .await;
                driver_state.task_registry.finalize(task_id);
                publish_roster(&driver_state, root).await;
            }
        });
        Ok(SpawnedSubagent {
            task_id: plan.task_id,
            child_session_id,
        })
    }
}
