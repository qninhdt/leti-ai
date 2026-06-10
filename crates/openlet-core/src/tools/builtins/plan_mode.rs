//! `enter_plan_mode` / `exit_plan_mode` tools — the only public path the
//! model has to flip the session's active agent profile.
//!
//! `EnterPlanMode` switches the session to the read-only `plan` profile
//! and emits `PlanModeEntered`. `ExitPlanMode` carries the model's
//! frozen plan text, restores the prior profile (or stays put when
//! `previous_agent_slug` is missing — no-op-with-event semantic),
//! emits `PlanModeExited`, and persists `Part::Plan` for audit/replay.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::adapters::event_sink::Persistence;
use crate::adapters::memory_store::MemoryStore;
use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::Tool;
use crate::types::event::AgentEvent;
use crate::types::message::{Message, MessageId, Role};
use crate::types::part::{Part, PartId};
use crate::types::permission::PermissionRequest;

/// Slug of the read-only plan-mode profile that `EnterPlanMode` flips
/// the session to. Mirrored in the `core-agents` plugin's `plan_agent`.
pub const PLAN_AGENT_SLUG: &str = "plan";

/// Default profile the runtime restores to when a session has never
/// switched profiles before (`previous_agent_slug` IS `None`). Matches
/// the `core-agents` plugin's `general_agent` slug.
pub const DEFAULT_AGENT_SLUG: &str = "general";

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct EnterPlanModeInput {}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EnterPlanModeOutput {
    /// Always `"plan"`. Returned so the model sees confirmation of the
    /// new profile in the tool result.
    pub agent: String,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ExitPlanModeInput {
    /// The full plan text the operator should review before
    /// implementation. Persisted as `Part::Plan` for audit/replay.
    pub plan: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ExitPlanModeOutput {
    /// Slug of the profile the session was restored to, or `"general"`
    /// when the session had no recorded prior profile (fallback).
    pub restored_agent: String,
    /// Whether the session was actually in plan mode at the time of
    /// the call. `false` ⇒ this was a naive call by the model;
    /// the event still fired so the operator sees the plan.
    pub was_in_plan_mode: bool,
}

pub struct EnterPlanModeTool {
    memory: Arc<dyn MemoryStore>,
}

impl EnterPlanModeTool {
    #[must_use]
    pub fn new(memory: Arc<dyn MemoryStore>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for EnterPlanModeTool {
    type Input = EnterPlanModeInput;
    type Output = EnterPlanModeOutput;

    fn name(&self) -> &'static str {
        "enter_plan_mode"
    }
    fn description(&self) -> &'static str {
        "Enter plan mode — switch the session to a read-only profile. Use sparingly: once entered, only the `exit_plan_mode` tool can leave."
    }
    fn parallel_safe(&self) -> bool {
        false
    }

    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest {
            permission: "agent:enter_plan_mode".into(),
            reason: None,
            timeout: None,
        }
    }

    async fn run(&self, ctx: ToolCtx, _: Self::Input) -> Result<Self::Output, ToolError> {
        self.memory
            .switch_agent(ctx.session_id, PLAN_AGENT_SLUG)
            .await
            .map_err(|e| ToolError::Io(format!("switch agent: {e}")))?;
        let _ = ctx
            .events
            .publish(
                AgentEvent::PlanModeEntered {
                    session_id: ctx.session_id,
                    at: Utc::now(),
                },
                Persistence::Durable,
            )
            .await;
        Ok(EnterPlanModeOutput {
            agent: PLAN_AGENT_SLUG.to_string(),
        })
    }
}

pub struct ExitPlanModeTool {
    memory: Arc<dyn MemoryStore>,
}

impl ExitPlanModeTool {
    #[must_use]
    pub fn new(memory: Arc<dyn MemoryStore>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    type Input = ExitPlanModeInput;
    type Output = ExitPlanModeOutput;

    fn name(&self) -> &'static str {
        "exit_plan_mode"
    }
    fn description(&self) -> &'static str {
        "Exit plan mode and submit the final plan. The session's active agent is restored to the profile it was on before EnterPlanMode."
    }
    fn parallel_safe(&self) -> bool {
        false
    }

    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest {
            permission: "agent:exit_plan_mode".into(),
            reason: None,
            timeout: None,
        }
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        let meta = self
            .memory
            .get_session(ctx.session_id)
            .await
            .map_err(|e| ToolError::Io(format!("read session: {e}")))?
            .ok_or_else(|| ToolError::Io("session not found".into()))?;
        let was_in_plan_mode = meta.current_agent_slug.as_deref() == Some(PLAN_AGENT_SLUG);
        // Restore prior slug — fall back to `general` so a naive
        // `ExitPlanMode` from outside plan mode still leaves the
        // session in a known good state. The event flow stays
        // symmetric: even a no-op exit publishes `PlanModeExited` so
        // the operator surfaces the plan.
        let restored = meta
            .previous_agent_slug
            .clone()
            .unwrap_or_else(|| DEFAULT_AGENT_SLUG.to_string());
        if was_in_plan_mode {
            self.memory
                .switch_agent(ctx.session_id, &restored)
                .await
                .map_err(|e| ToolError::Io(format!("restore agent: {e}")))?;
        }
        persist_plan_part(self.memory.as_ref(), ctx.session_id, &input.plan)
            .await
            .map_err(|e| ToolError::Io(format!("persist plan: {e}")))?;
        let _ = ctx
            .events
            .publish(
                AgentEvent::PlanModeExited {
                    session_id: ctx.session_id,
                    plan: input.plan.clone(),
                    at: Utc::now(),
                },
                Persistence::Durable,
            )
            .await;
        Ok(ExitPlanModeOutput {
            restored_agent: restored,
            was_in_plan_mode,
        })
    }
}

/// Append a fresh tool-role message holding `Part::Plan` so the plan
/// survives session reload. Kept as a free function so both the tool
/// and any future migration helper share the format.
async fn persist_plan_part(
    memory: &dyn MemoryStore,
    session_id: crate::types::session::SessionId,
    plan: &str,
) -> Result<(), crate::error::MemoryError> {
    let msg = Message {
        id: MessageId::new(),
        session_id,
        role: Role::Tool,
        created_at: Utc::now(),
    };
    let mid = memory.append_message(session_id, msg).await?;
    memory
        .append_part(
            mid,
            Part::Plan {
                id: PartId::new(),
                plan: plan.to_string(),
            },
        )
        .await?;
    Ok(())
}
