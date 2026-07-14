//! `promote_task` tool — mark a background subagent for auto-notification.
//!
//! Phase 3 execution model (red-team-corrected): a blocking sync
//! `subagent_task` call CANNOT coexist with a concurrent `promote_task`
//! in one single-threaded turn loop, so promotion does NOT try to unblock
//! a parked sync call. Instead the model:
//!   1. spawns work with `subagent_task { background: true }` (returns a
//!      `task_id` immediately, parent continues),
//!   2. optionally calls `promote_task { task_id }` so the result is
//!      AUTO-INJECTED into the parent conversation on settle (via the
//!      Phase 2 turn queue) instead of requiring a `task_status` poll.
//!
//! Promoting a task only sets a flag (`was_promoted`); the driver's
//! settle path (Phase 3 `drive_subagent`) reads it and routes the output
//! through the parent injector. Promoting an unknown / already-finalized
//! task is a typed no-op (never an error) so a model that races settle
//! gets a clean acknowledgement.

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
pub struct PromoteTaskInput {
    /// UUIDv4 string of a background task previously returned by
    /// `subagent_task { background: true }`. Invalid / unknown ids are a
    /// no-op acknowledgement rather than an error.
    pub task_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PromoteTaskOutput {
    pub task_id: String,
    /// `true` when the task was found and marked promoted; `false` when
    /// the id was unknown / already finalized (still an ack, not an error).
    pub promoted: bool,
}

pub struct PromoteTaskTool {
    registry: Arc<TaskRegistry>,
}

impl PromoteTaskTool {
    #[must_use]
    pub fn new(registry: Arc<TaskRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for PromoteTaskTool {
    type Input = PromoteTaskInput;
    type Output = PromoteTaskOutput;

    fn name(&self) -> &'static str {
        "promote_task"
    }
    fn description(&self) -> &'static str {
        "Promote a background subagent task (from subagent_task with background=true) so its \
         result is automatically delivered back into this conversation when it finishes, instead \
         of requiring a task_status poll. Call after spawning background work you want to be \
         notified about."
    }
    fn parallel_safe(&self) -> bool {
        true
    }

    fn permission(&self, _input: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple("promote_task")
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        let Ok(uuid) = Uuid::parse_str(&input.task_id) else {
            return Ok(PromoteTaskOutput {
                task_id: input.task_id,
                promoted: false,
            });
        };
        let promoted = self.registry.mark_promoted(TaskId(uuid));
        if promoted {
            // Announce the promotion so SSE consumers (TUI task panel)
            // can mark the row auto-notified. Best-effort — a dropped
            // frame doesn't change the delivery guarantee (the injected
            // result turn is the canonical carrier).
            use crate::adapters::event_sink::Persistence;
            use crate::types::event::AgentEvent;
            let _ = ctx
                .events
                .publish(
                    AgentEvent::SubagentPromoted {
                        task_id: uuid,
                        parent_session_id: ctx.session_id,
                    },
                    Persistence::Durable,
                )
                .await;
        }
        Ok(PromoteTaskOutput {
            task_id: input.task_id,
            promoted,
        })
    }
}
