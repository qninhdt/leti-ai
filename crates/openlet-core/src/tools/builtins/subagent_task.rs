//! `subagent_task` tool — spawn an in-process nested subagent.
//!
//! The tool's `run` is the boundary between the model-facing JSON shape
//! and the [`runtime::subagent`] state machine. Sync mode awaits
//! completion and returns a final `{output, cost_usd}`. Background mode
//! returns a task/child-session descriptor immediately; terminal output is
//! delivered once through a typed parent reminder (while `task_status` stays
//! available for inspection).
//!
//! Quotas + depth caps live in `runtime::subagent::plan_subagent_spawn`;
//! this tool only converts admit errors to typed [`ToolError`] variants.
//!
//! Spawn-driver wiring is intentionally NOT performed here — the actual
//! `ConversationRuntime::run_loop` call requires `AppState` plumbing
//! (memory, provider, tool registry, agent resources) that lives in the
//! server crate. This tool registers a `SubagentSpawner` callback in the
//! `ToolCtx` and delegates the heavy lifting.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::runtime::subagent::{SpawnError, TaskId, TaskStatus};
use crate::tools::{PromptPolicy, Tool};
use crate::types::permission::PermissionRequest;
use crate::types::session::SessionId;

/// Driver hook the server crate installs at boot. Given a resolved
/// spawn plan + `task_id`, it kicks off a nested
/// `ConversationRuntime::run_loop` and returns a future that completes
/// when the child finishes. Trait-object form keeps `openlet-core` free
/// of route/state deps.
#[async_trait]
pub trait SubagentSpawner: Send + Sync + 'static {
    /// Resolve the slug + objective into a running task. The
    /// implementation owns the depth/quota check (via
    /// `runtime::subagent::plan_subagent_spawn`) and tokio::spawn of
    /// the driver task. Returns the new `task_id` synchronously so the
    /// caller can poll/cancel even before the child completes.
    async fn spawn(
        &self,
        ctx: &ToolCtx,
        subagent_type: &str,
        objective: &str,
        scope: Option<&str>,
        background: bool,
    ) -> Result<SpawnedSubagent, SpawnError>;

    /// Await terminal status for `task_id`. Returns the final output
    /// and accumulated cost (rendered as a 4-decimal USD string).
    async fn await_completion(
        &self,
        task_id: TaskId,
    ) -> Result<(String, Option<String>, TaskStatus), SpawnError>;

    /// Wait for the original foreground invocation. Implementations may return
    /// a running acknowledgement when the TUI atomically backgrounds that
    /// invocation; explicit resume/poll callers keep using `await_completion`.
    async fn await_foreground_completion(
        &self,
        task_id: TaskId,
    ) -> Result<(String, Option<String>, TaskStatus), SpawnError> {
        self.await_completion(task_id).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnedSubagent {
    pub task_id: TaskId,
    pub child_session_id: SessionId,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SubagentTaskInput {
    /// Slug of the subagent definition to spawn. Omit to use `general`.
    /// When supplied, this must exactly match a registered agent slug.
    #[serde(default)]
    pub subagent_type: Option<String>,
    /// Plain-text instruction for the subagent's first user turn.
    pub objective: String,
    /// Optional working scope hint (e.g. file path or directory).
    /// Currently passed through unchanged for the subagent's prompt.
    #[serde(default)]
    pub scope: Option<String>,
    /// `true` returns immediately with `{task_id, child_session_id,
    /// status: "running"}` and schedules typed completion delivery. Default
    /// = false (foreground join).
    #[serde(default)]
    pub background: bool,
    /// Resume marker — the model can submit the previously-issued
    /// `task_id` (UUIDv4 string) to await an existing in-flight task
    /// without spawning a new one. Invalid UUIDs are ignored and a
    /// fresh spawn is attempted.
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SubagentTaskOutput {
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_session_id: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<String>,
}

pub struct SubagentTaskTool {
    spawner: Arc<dyn SubagentSpawner>,
}

impl SubagentTaskTool {
    #[must_use]
    pub fn new(spawner: Arc<dyn SubagentSpawner>) -> Self {
        Self { spawner }
    }
}

fn map_spawn_err(e: SpawnError) -> ToolError {
    // Surface the structured class via the InvalidInput message so the
    // model sees a stable error code in the tool result. The caller
    // (turn loop) wraps this into a Part::ToolResult { error } the
    // subsequent assistant turn observes.
    ToolError::InvalidInput(format!("{}: {}", e.code(), e))
}

#[async_trait]
impl Tool for SubagentTaskTool {
    type Input = SubagentTaskInput;
    type Output = SubagentTaskOutput;

    fn name(&self) -> &'static str {
        "subagent_task"
    }
    fn description(&self) -> &'static str {
        "Spawn a nested subagent session. Omit subagent_type to use general. Sync by default; pass \
         background=true to run async and poll via task_status. Bounded by per-session depth + quota."
    }
    fn parallel_safe(&self) -> bool {
        true
    }
    fn prompt_policy(&self) -> PromptPolicy {
        PromptPolicy::ContinueOnAsk
    }

    fn permission(&self, input: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple(format!(
            "subagent_task:{}",
            input.subagent_type.as_deref().unwrap_or("general")
        ))
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        // Resume path — caller provided an existing task_id. Skip spawn,
        // jump straight to await/poll.
        if let Some(existing) = input.task_id.as_deref()
            && let Ok(uuid) = uuid::Uuid::parse_str(existing)
        {
            let id = TaskId(uuid);
            if input.background {
                return Ok(SubagentTaskOutput {
                    task_id: existing.to_string(),
                    child_session_id: ctx
                        .task_registry
                        .child_session(id)
                        .map(|session| session.to_string()),
                    status: "running".into(),
                    output: None,
                    cost_usd: None,
                });
            }
            let (output, cost, status) = self
                .spawner
                .await_completion(id)
                .await
                .map_err(map_spawn_err)?;
            return Ok(SubagentTaskOutput {
                task_id: existing.to_string(),
                child_session_id: ctx
                    .task_registry
                    .child_session(id)
                    .map(|session| session.to_string()),
                status: status.label().to_string(),
                output: Some(output),
                cost_usd: cost,
            });
        }

        // Omission has one documented meaning: use the registered `general`
        // agent. An explicit type is passed through unchanged and validated by
        // the spawner; unknown names never silently become another agent.
        let subagent_type = input.subagent_type.as_deref().unwrap_or("general");
        let spawned = self
            .spawner
            .spawn(
                &ctx,
                subagent_type,
                &input.objective,
                input.scope.as_deref(),
                input.background,
            )
            .await
            .map_err(map_spawn_err)?;

        if input.background {
            return Ok(SubagentTaskOutput {
                task_id: spawned.task_id.0.to_string(),
                child_session_id: Some(spawned.child_session_id.to_string()),
                status: "running".into(),
                output: None,
                cost_usd: None,
            });
        }

        let (output, cost, status) = self
            .spawner
            .await_foreground_completion(spawned.task_id)
            .await
            .map_err(map_spawn_err)?;
        Ok(SubagentTaskOutput {
            task_id: spawned.task_id.0.to_string(),
            child_session_id: Some(spawned.child_session_id.to_string()),
            status: status.label().to_string(),
            output: Some(output),
            cost_usd: cost,
        })
    }
}
