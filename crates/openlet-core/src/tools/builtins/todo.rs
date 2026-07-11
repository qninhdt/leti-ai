//! `todo` tool — per-session checklist storage via the artifact store.
//!
//! Full-list overwrite, statuses
//! `pending|in_progress|completed|cancelled`, priority enum required.
//! Storage key: `todos.json` under the session's artifact namespace.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::Tool;
use crate::types::permission::PermissionRequest;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TodoPriority {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
    pub priority: TodoPriority,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct TodoInput {
    pub todos: Vec<TodoItem>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TodoOutput {
    pub count: usize,
    pub incomplete: usize,
}

pub struct TodoTool;

#[async_trait]
impl Tool for TodoTool {
    type Input = TodoInput;
    type Output = TodoOutput;

    fn name(&self) -> &'static str {
        "todo"
    }
    fn description(&self) -> &'static str {
        "Replace the session's todo list. Statuses: pending|in_progress|completed|cancelled."
    }
    fn parallel_safe(&self) -> bool {
        true
    }

    fn permission(&self, _input: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple("todo")
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        let count = input.todos.len();
        let incomplete = input
            .todos
            .iter()
            .filter(|t| matches!(t.status, TodoStatus::Pending | TodoStatus::InProgress))
            .count();
        let payload =
            serde_json::to_vec_pretty(&input.todos).map_err(|e| ToolError::Io(e.to_string()))?;
        let bytes = Bytes::from(payload);
        let _ = upload_todos(&ctx, bytes).await?;
        Ok(TodoOutput { count, incomplete })
    }
}

async fn upload_todos(ctx: &ToolCtx, bytes: Bytes) -> Result<(), ToolError> {
    use crate::adapters::artifact_store::ArtifactStore;
    let store: Arc<dyn ArtifactStore> = ctx.artifacts.clone();
    store
        .put(ctx.session_id, "todos.json", bytes)
        .await
        .map_err(|e| ToolError::Io(e.to_string()))?;
    Ok(())
}
