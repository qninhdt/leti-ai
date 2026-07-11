//! `write` tool — file write with read-before-write gate, atomic via
//! `ctx.fs.write` with `WriteOpts::atomic = true`.

use std::path::PathBuf;

use crate::adapters::filesystem::WriteOpts;
use async_trait::async_trait;
use bytes::Bytes;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::Tool;
use crate::types::permission::{PermissionMode, PermissionRequest};

const MAX_WRITE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WriteInput {
    pub path: PathBuf,
    /// Full new contents of the file. UTF-8.
    pub content: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WriteOutput {
    pub path: String,
    pub bytes_written: usize,
    pub kind: &'static str, // "create" | "update"
    /// Line-level diff of an update (old → new). `None` on create (no
    /// prior content). Rides through the tool-result JSON body — no
    /// protocol change (see `tools::diff`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<crate::tools::diff::FileDiff>,
}

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    type Input = WriteInput;
    type Output = WriteOutput;

    fn name(&self) -> &'static str {
        "write"
    }
    fn description(&self) -> &'static str {
        "Write the full contents of a file. Existing files must be read first (unless mode=danger)."
    }
    fn parallel_safe(&self) -> bool {
        false
    }

    fn permission(&self, input: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple(format!("edit:{}", input.path.display()))
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        if input.content.len() > MAX_WRITE_BYTES {
            return Err(ToolError::FileTooLarge {
                path: input.path.display().to_string(),
                bytes: input.content.len() as u64,
                limit: MAX_WRITE_BYTES as u64,
            });
        }
        let exists = ctx.fs.exists(&input.path).await;
        if exists
            && !ctx.mode.permits(PermissionMode::Danger)
            && !ctx.read_history.contains(&input.path).await
        {
            return Err(ToolError::ReadBeforeWriteRequired(
                input.path.display().to_string(),
            ));
        }

        // On update, read the prior content for a diff before overwriting.
        // Best-effort: a read failure or non-UTF-8 original simply yields no
        // diff rather than failing the write. Skipped entirely on create.
        let original = if exists {
            match ctx.fs.read(&input.path, None).await {
                Ok(prev) => String::from_utf8(prev.to_vec()).ok(),
                Err(_) => None,
            }
        } else {
            None
        };

        let bytes = input.content.into_bytes();
        let body = Bytes::from(bytes.clone());
        let _meta = ctx
            .fs
            .write(&input.path, body, WriteOpts::default())
            .await?;

        // Record so subsequent edits in the same session don't trip the gate.
        ctx.read_history.record(input.path.clone()).await;

        let new_content = String::from_utf8(bytes.clone()).ok();
        let diff = match (original, new_content) {
            (Some(old), Some(new)) => Some(crate::tools::diff::compute_line_diff(
                &old,
                &new,
                crate::tools::diff::DEFAULT_LINE_CAP,
            )),
            _ => None,
        };

        Ok(WriteOutput {
            path: input.path.display().to_string(),
            bytes_written: bytes.len(),
            kind: if exists { "update" } else { "create" },
            diff,
        })
    }
}
