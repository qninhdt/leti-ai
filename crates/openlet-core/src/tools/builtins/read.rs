//! `read` tool — line-numbered file reader with byte cap.
//!
//! Bytes come from `ctx.fs.read(...)`; this tool only owns the
//! line-number formatting + line/output caps + read-history record.

use std::path::PathBuf;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::Tool;
use crate::types::permission::PermissionRequest;

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const DEFAULT_READ_LIMIT: usize = 2000;
const MAX_LINE_LENGTH: usize = 2000;
const MAX_OUTPUT_BYTES: usize = 50 * 1024;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ReadInput {
    /// Workspace-relative path (or absolute under workspace root).
    pub path: PathBuf,
    /// 1-based line number to start at. Defaults to 1.
    #[serde(default)]
    pub offset: Option<usize>,
    /// Max lines to return. Defaults to 2000.
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ReadOutput {
    pub path: String,
    pub content: String,
    pub line_count: usize,
    pub truncated: bool,
}

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    type Input = ReadInput;
    type Output = ReadOutput;

    fn name(&self) -> &'static str {
        "read"
    }
    fn description(&self) -> &'static str {
        "Read a UTF-8 text file from the workspace. Returns line-numbered content with pagination."
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
        let meta = ctx.fs.stat(&input.path).await?;
        if meta.size > MAX_FILE_BYTES {
            return Err(ToolError::FileTooLarge {
                path: input.path.display().to_string(),
                bytes: meta.size,
                limit: MAX_FILE_BYTES,
            });
        }
        if meta.is_binary {
            return Err(ToolError::BinaryFile(input.path.display().to_string()));
        }

        let bytes = ctx.fs.read(&input.path, None).await?;
        let text = String::from_utf8_lossy(&bytes);

        let offset = input.offset.unwrap_or(1).max(1);
        let limit = input.limit.unwrap_or(DEFAULT_READ_LIMIT);

        let mut lines: Vec<String> = Vec::new();
        let mut total_bytes = 0usize;
        let mut truncated = false;

        for (idx, raw) in text.split_inclusive('\n').enumerate() {
            let line_no = idx + 1;
            if line_no < offset {
                continue;
            }
            if lines.len() >= limit {
                truncated = true;
                break;
            }
            let trimmed = raw.strip_suffix('\n').unwrap_or(raw);
            let body = if trimmed.len() > MAX_LINE_LENGTH {
                format!(
                    "{}... (line truncated to {MAX_LINE_LENGTH} chars)",
                    &trimmed[..MAX_LINE_LENGTH]
                )
            } else {
                trimmed.to_string()
            };
            let formatted = format!("{line_no}: {body}\n");
            if total_bytes + formatted.len() > MAX_OUTPUT_BYTES {
                truncated = true;
                break;
            }
            total_bytes += formatted.len();
            lines.push(formatted);
        }

        let content = lines.concat();
        let line_count = lines.len();

        // Record in read_history (workspace-relative path) so write/edit
        // can gate later in the same session.
        ctx.read_history.record(input.path.clone()).await;

        Ok(ReadOutput {
            path: input.path.display().to_string(),
            content,
            line_count,
            truncated,
        })
    }
}
