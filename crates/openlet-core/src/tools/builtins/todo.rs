//! `todo` tool — per-session checklist storage via the artifact store.
//!
//! Full-list overwrite, statuses `pending|in_progress|completed`,
//! priority enum required. Storage key: `todos.json` under the session's
//! artifact namespace. Cancellation is expressed by OMISSION on the next
//! overwrite (there is no `cancelled` status) — matching the Claude Code /
//! codex doctrine every reference agent agrees on.
//!
//! After a confirmed persist the tool publishes a `todo.updated` event so
//! the TUI re-renders the checklist live. The tool is `parallel_safe=false`
//! because it mutates shared session state at a fixed artifact key: two
//! concurrent calls in one dispatch wave would race on the same path and
//! emit events out of order.

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::adapters::event_sink::Persistence;
use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::Tool;
use crate::types::event::{AgentEvent, TodoEventItem};
use crate::types::permission::PermissionRequest;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    /// The snake_case wire string, matching the serde representation.
    fn wire(self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoPriority {
    High,
    Medium,
    Low,
}

impl TodoPriority {
    fn wire(self) -> &'static str {
        match self {
            TodoPriority::High => "high",
            TodoPriority::Medium => "medium",
            TodoPriority::Low => "low",
        }
    }
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
    /// Number of items currently `in_progress`. Advisory only — the tool
    /// does not reject multiple in-progress items (no reference agent does).
    pub in_progress: usize,
}

const TODO_DESCRIPTION: &str = "\
Create and maintain a structured task list for the current coding session. \
Replaces the whole list every call (full overwrite — send the complete list, \
not a delta).

## When to use
- The task needs 3+ distinct steps, or the user gave several tasks.
- Non-trivial work that benefits from an explicit plan.
- Mark exactly ONE item `in_progress` before starting it; mark it `completed` \
the moment it is done — do not batch completions.

## When NOT to use
- A single, trivial step, or a purely informational/conversational request.

## States
- `pending` — not started.
- `in_progress` — actively working (keep to one at a time).
- `completed` — finished successfully.
- To cancel or drop a task, OMIT it from the next overwrite. There is no \
`cancelled` status.

## Fields
Each item is `{ content, status, priority }`; priority is `high|medium|low`.

## Rules
- Update status in real time; keep exactly one `in_progress` while work remains.
- Mark `completed` only after the work is actually done, not on intent.

## Example
`{ \"todos\": [ { \"content\": \"Add auth\", \"status\": \"in_progress\", \
\"priority\": \"high\" }, { \"content\": \"Write tests\", \"status\": \
\"pending\", \"priority\": \"medium\" } ] }`";

pub struct TodoTool;

#[async_trait]
impl Tool for TodoTool {
    type Input = TodoInput;
    type Output = TodoOutput;

    fn name(&self) -> &'static str {
        "todo"
    }
    fn description(&self) -> &'static str {
        TODO_DESCRIPTION
    }
    fn parallel_safe(&self) -> bool {
        // Mutates shared session state at a fixed artifact key — two
        // concurrent calls in one wave would race the write + reorder events.
        false
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
        let in_progress = input
            .todos
            .iter()
            .filter(|t| t.status == TodoStatus::InProgress)
            .count();

        let payload =
            serde_json::to_vec_pretty(&input.todos).map_err(|e| ToolError::Io(e.to_string()))?;
        let bytes = Bytes::from(payload);
        // `ArtifactStore::put` is now atomic (tempfile + rename), so a crash
        // mid-write can never leave a torn `todos.json`. Only publish the
        // event AFTER this confirmed persist.
        upload_todos(&ctx, bytes).await?;

        let items: Vec<TodoEventItem> = input
            .todos
            .iter()
            .map(|t| TodoEventItem {
                content: t.content.clone(),
                status: t.status.wire().to_string(),
                priority: t.priority.wire().to_string(),
            })
            .collect();
        // Best-effort: a bus error must not fail the tool after the durable
        // write already landed. The list is safely persisted regardless.
        let _ = ctx
            .events
            .publish(
                AgentEvent::TodoUpdated {
                    session_id: ctx.session_id,
                    items,
                },
                Persistence::Durable,
            )
            .await;

        Ok(TodoOutput {
            count,
            incomplete,
            in_progress,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_has_exactly_three_variants_snake_case() {
        assert_eq!(
            serde_json::to_string(&TodoStatus::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&TodoStatus::InProgress).unwrap(),
            "\"in_progress\""
        );
        assert_eq!(
            serde_json::to_string(&TodoStatus::Completed).unwrap(),
            "\"completed\""
        );
        // A dropped `cancelled` value must no longer deserialize.
        assert!(serde_json::from_str::<TodoStatus>("\"cancelled\"").is_err());
    }

    #[test]
    fn wire_strings_match_serde() {
        for (s, want) in [
            (TodoStatus::Pending, "pending"),
            (TodoStatus::InProgress, "in_progress"),
            (TodoStatus::Completed, "completed"),
        ] {
            assert_eq!(s.wire(), want);
            assert_eq!(serde_json::to_string(&s).unwrap(), format!("\"{want}\""));
        }
        for (p, want) in [
            (TodoPriority::High, "high"),
            (TodoPriority::Medium, "medium"),
            (TodoPriority::Low, "low"),
        ] {
            assert_eq!(p.wire(), want);
            assert_eq!(serde_json::to_string(&p).unwrap(), format!("\"{want}\""));
        }
    }
}
