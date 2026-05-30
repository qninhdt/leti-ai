//! `task_status` tool — read-only poll of a running subagent task.
//!
//! The model uses this to drive a "fan-out + join" workflow: spawn N
//! background subagents via `subagent_task { background: true }`, then
//! poll each `task_id` until terminal. The tool is parallel-safe — it's
//! a `DashMap` lookup and three reads of `Arc<RwLock<_>>`.
//!
//! No spawn side effects: a `task_id` for an unknown task returns a
//! stable `not_found` status rather than an error so a model that
//! polls a task it never started gets a clean signal.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::runtime::subagent::{TaskId, TaskRegistry};
use crate::tools::Tool;
use crate::types::permission::PermissionRequest;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TaskStatusInput {
    /// UUIDv4 string returned by `subagent_task`. Invalid UUIDs return
    /// a `not_found` status rather than an error.
    pub task_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TaskStatusOutput {
    pub task_id: String,
    /// One of: `running`, `finished`, `cancelled`, `failed`, `not_found`.
    pub status: String,
    /// Output buffered so far. Bounded by the per-task 10MB cap; once
    /// exceeded, replaced with the literal `[truncated]` sentinel.
    pub output_so_far: String,
    /// Accumulated USD cost rendered as a 4-decimal string. Empty when
    /// no provider call has been billed yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<String>,
    /// `true` when status is one of `finished | cancelled | failed`.
    pub finished: bool,
    /// Optional failure feedback when status is `failed`. Populated from
    /// the underlying `TaskStatus::Failed` payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub struct TaskStatusTool {
    registry: Arc<TaskRegistry>,
}

impl TaskStatusTool {
    #[must_use]
    pub fn new(registry: Arc<TaskRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for TaskStatusTool {
    type Input = TaskStatusInput;
    type Output = TaskStatusOutput;

    fn name(&self) -> &'static str {
        "task_status"
    }
    fn description(&self) -> &'static str {
        "Poll a previously-issued subagent task by id. Returns status, output buffered so far, \
         and accumulated cost. Cheap; safe to call repeatedly."
    }
    fn parallel_safe(&self) -> bool {
        true
    }

    fn permission(&self, _input: &Self::Input) -> PermissionRequest {
        PermissionRequest {
            permission: "task_status".to_string(),
            reason: None,
            timeout: None,
        }
    }

    async fn run(&self, _ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        let not_found = |task_id: String| TaskStatusOutput {
            task_id,
            status: "not_found".to_string(),
            output_so_far: String::new(),
            cost_usd: None,
            finished: true,
            error: None,
        };

        let Ok(uuid) = Uuid::parse_str(&input.task_id) else {
            return Ok(not_found(input.task_id));
        };
        let id = TaskId(uuid);
        let Some(snap) = self.registry.poll_async(id).await else {
            return Ok(not_found(input.task_id));
        };
        let cost = if snap.cost_usd.is_zero() {
            None
        } else {
            Some(crate::runtime::cost::format_usd(snap.cost_usd))
        };
        let error = match &snap.status {
            crate::runtime::subagent::TaskStatus::Failed(msg) => Some(msg.clone()),
            _ => None,
        };
        Ok(TaskStatusOutput {
            task_id: input.task_id,
            status: snap.status.label().to_string(),
            output_so_far: snap.output,
            cost_usd: cost,
            finished: snap.finished,
            error,
        })
    }
}
