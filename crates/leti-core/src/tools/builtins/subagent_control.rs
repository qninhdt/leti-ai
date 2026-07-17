//! Read and lifecycle controls for durable subagent executions.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::adapters::memory_store::MemoryStore;
use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::runtime::subagent::{TaskId, TaskRegistry};
use crate::tools::Tool;
use crate::tools::builtins::subagent_task::SubagentSpawner;
use crate::types::permission::PermissionRequest;
use crate::types::session::SessionId;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SubagentExecutionOutput {
    pub task_id: String,
    pub child_session_id: String,
    pub agent_slug: String,
    pub objective: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<String>,
}

impl From<crate::runtime::subagent::SubagentExecution> for SubagentExecutionOutput {
    fn from(value: crate::runtime::subagent::SubagentExecution) -> Self {
        Self {
            task_id: value.task_id.to_string(),
            child_session_id: value.child_session_id.to_string(),
            agent_slug: value.agent_slug,
            objective: value.objective,
            status: value.status.label().to_string(),
            terminal_reason: value.terminal_reason,
            cost_usd: value.cost_usd,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SubagentListInput {
    #[serde(default)]
    pub include_terminal: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SubagentListOutput {
    pub executions: Vec<SubagentExecutionOutput>,
}

pub struct SubagentListTool {
    memory: Arc<dyn MemoryStore>,
}

impl SubagentListTool {
    #[must_use]
    pub fn new(memory: Arc<dyn MemoryStore>) -> Self {
        Self { memory }
    }
}

#[async_trait]
impl Tool for SubagentListTool {
    type Input = SubagentListInput;
    type Output = SubagentListOutput;

    fn name(&self) -> &'static str {
        "subagent_list"
    }
    fn description(&self) -> &'static str {
        "List this conversation root's subagent executions. Terminal executions are omitted unless include_terminal is true."
    }
    fn parallel_safe(&self) -> bool {
        true
    }
    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple("subagent_list")
    }
    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        let root = root_of(&*self.memory, ctx.session_id).await?;
        let executions = self
            .memory
            .list_subagent_executions(root, input.include_terminal)
            .await
            .map_err(|e| ToolError::Io(format!("subagent_list: {e}")))?
            .into_iter()
            .map(Into::into)
            .collect();
        Ok(SubagentListOutput { executions })
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SubagentControlInput {
    pub task_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SubagentControlOutput {
    pub task_id: String,
    pub status: String,
}

pub struct SubagentCancelTool {
    memory: Arc<dyn MemoryStore>,
    registry: Arc<TaskRegistry>,
}

impl SubagentCancelTool {
    #[must_use]
    pub fn new(memory: Arc<dyn MemoryStore>, registry: Arc<TaskRegistry>) -> Self {
        Self { memory, registry }
    }
}

#[async_trait]
impl Tool for SubagentCancelTool {
    type Input = SubagentControlInput;
    type Output = SubagentControlOutput;
    fn name(&self) -> &'static str {
        "subagent_cancel"
    }
    fn description(&self) -> &'static str {
        "Cancel a live subagent execution in this conversation root. Repeated calls are safe."
    }
    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple("subagent_cancel")
    }
    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        control(
            &*self.memory,
            &self.registry,
            ctx.session_id,
            &input.task_id,
            false,
        )
        .await
    }
}

pub struct SubagentInterruptTool {
    memory: Arc<dyn MemoryStore>,
    registry: Arc<TaskRegistry>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SubagentContinueInput {
    pub child_session_id: String,
    pub objective: String,
    #[serde(default)]
    pub background: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SubagentContinueOutput {
    pub task_id: String,
    pub child_session_id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<String>,
}

pub struct SubagentContinueTool {
    spawner: Arc<dyn SubagentSpawner>,
}

impl SubagentContinueTool {
    #[must_use]
    pub fn new(spawner: Arc<dyn SubagentSpawner>) -> Self {
        Self { spawner }
    }
}

#[async_trait]
impl Tool for SubagentContinueTool {
    type Input = SubagentContinueInput;
    type Output = SubagentContinueOutput;
    fn name(&self) -> &'static str {
        "subagent_continue"
    }
    fn description(&self) -> &'static str {
        "Start a new execution on an interrupted or completed subagent child session, preserving that child's transcript."
    }
    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple("subagent_continue")
    }
    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        let child_session_id = Uuid::parse_str(&input.child_session_id)
            .map(SessionId)
            .map_err(|_| {
                ToolError::InvalidInput("subagent_continue: child_session_id must be UUIDv4".into())
            })?;
        let spawned = self
            .spawner
            .continue_subagent(&ctx, child_session_id, &input.objective, input.background)
            .await
            .map_err(|e| ToolError::InvalidInput(format!("{}: {e}", e.code())))?;
        if input.background {
            return Ok(SubagentContinueOutput {
                task_id: spawned.task_id.to_string(),
                child_session_id: spawned.child_session_id.to_string(),
                status: "running".into(),
                output: None,
                cost_usd: None,
            });
        }
        let (output, cost_usd, status) = self
            .spawner
            .await_foreground_completion(spawned.task_id)
            .await
            .map_err(|e| ToolError::InvalidInput(format!("{}: {e}", e.code())))?;
        Ok(SubagentContinueOutput {
            task_id: spawned.task_id.to_string(),
            child_session_id: spawned.child_session_id.to_string(),
            status: status.label().into(),
            output: Some(output),
            cost_usd,
        })
    }
}

impl SubagentInterruptTool {
    #[must_use]
    pub fn new(memory: Arc<dyn MemoryStore>, registry: Arc<TaskRegistry>) -> Self {
        Self { memory, registry }
    }
}

#[async_trait]
impl Tool for SubagentInterruptTool {
    type Input = SubagentControlInput;
    type Output = SubagentControlOutput;
    fn name(&self) -> &'static str {
        "subagent_interrupt"
    }
    fn description(&self) -> &'static str {
        "Interrupt a live subagent execution but preserve its child session for subagent_continue."
    }
    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple("subagent_interrupt")
    }
    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        control(
            &*self.memory,
            &self.registry,
            ctx.session_id,
            &input.task_id,
            true,
        )
        .await
    }
}

async fn control(
    memory: &dyn MemoryStore,
    registry: &TaskRegistry,
    requester: SessionId,
    raw_task_id: &str,
    interrupt: bool,
) -> Result<SubagentControlOutput, ToolError> {
    let uuid = Uuid::parse_str(raw_task_id)
        .map_err(|_| ToolError::InvalidInput("subagent task_id must be UUIDv4".into()))?;
    let task_id = TaskId(uuid);
    let execution = memory
        .get_subagent_execution(task_id)
        .await
        .map_err(|e| ToolError::Io(format!("subagent control: {e}")))?
        .ok_or_else(|| ToolError::InvalidInput("subagent task not found".into()))?;
    if root_of(memory, requester).await? != execution.root_session_id {
        return Err(ToolError::PermissionDenied(
            "subagent task belongs to another root session".into(),
        ));
    }
    if execution.status.is_terminal() {
        return Ok(SubagentControlOutput {
            task_id: raw_task_id.to_string(),
            status: execution.status.label().into(),
        });
    }
    if interrupt {
        registry.interrupt(task_id);
        Ok(SubagentControlOutput {
            task_id: raw_task_id.to_string(),
            status: "interrupting".into(),
        })
    } else {
        registry.cancel(task_id);
        Ok(SubagentControlOutput {
            task_id: raw_task_id.to_string(),
            status: "cancelling".into(),
        })
    }
}

async fn root_of(memory: &dyn MemoryStore, session: SessionId) -> Result<SessionId, ToolError> {
    let mut current = session;
    for _ in 0..8 {
        let meta = memory
            .get_session(current)
            .await
            .map_err(|e| ToolError::Io(format!("subagent root lookup: {e}")))?;
        match meta.and_then(|m| m.parent_session_id) {
            Some(parent) => current = parent,
            None => return Ok(current),
        }
    }
    Err(ToolError::InvalidInput(
        "subagent parent chain exceeds maximum depth".into(),
    ))
}
