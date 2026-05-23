//! `list` tool — shallow directory listing via `ctx.fs.list`.

use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::Tool;
use crate::types::permission::PermissionRequest;

const MAX_ENTRIES: usize = 1000;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ListInput {
    /// Workspace-relative directory.
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListEntry {
    pub name: String,
    pub kind: &'static str, // "file" | "dir"
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ListOutput {
    pub path: String,
    pub entries: Vec<ListEntry>,
    pub truncated: bool,
}

pub struct ListTool;

#[async_trait]
impl Tool for ListTool {
    type Input = ListInput;
    type Output = ListOutput;

    fn name(&self) -> &'static str {
        "list"
    }
    fn description(&self) -> &'static str {
        "List the immediate entries of a directory (non-recursive)."
    }
    fn parallel_safe(&self) -> bool {
        true
    }

    fn permission(&self, input: &Self::Input) -> PermissionRequest {
        PermissionRequest {
            permission: format!("read:{}", input.path.display()),
            reason: None,
            timeout: None,
        }
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        let raw = ctx.fs.list(&input.path).await?;
        let truncated = raw.len() > MAX_ENTRIES;
        let entries: Vec<ListEntry> = raw
            .into_iter()
            .take(MAX_ENTRIES)
            .map(|e| ListEntry {
                name: e.name,
                kind: if e.is_dir { "dir" } else { "file" },
                size: e.size,
            })
            .collect();

        Ok(ListOutput {
            path: input.path.display().to_string(),
            entries,
            truncated,
        })
    }
}
