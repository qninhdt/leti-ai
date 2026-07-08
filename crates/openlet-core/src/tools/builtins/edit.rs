//! `edit` tool — find/replace with read-before-write + unique-match gate.
//!
//! Single-replace mode requires the `find` text to appear exactly once
//! (Anthropic str_replace semantics; tighter than a
//! `first_index == last_index` shortcut). `replace_all` switches to
//! verbatim `String::replace`. We reject ambiguous matches (rather than
//! replacing the first) because they silently corrupt files.

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

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct EditInput {
    pub path: PathBuf,
    pub find: String,
    pub replace: String,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct EditOutput {
    pub path: String,
    pub replacements: usize,
    /// Line-level diff of the change, for TUI rendering. Rides through the
    /// tool-result JSON body — no protocol change (see `tools::diff`).
    pub diff: crate::tools::diff::FileDiff,
}

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    type Input = EditInput;
    type Output = EditOutput;

    fn name(&self) -> &'static str {
        "edit"
    }
    fn description(&self) -> &'static str {
        "Find/replace inside a file. Single-match by default; pass replace_all=true for global. Read first."
    }
    fn parallel_safe(&self) -> bool {
        false
    }

    fn permission(&self, input: &Self::Input) -> PermissionRequest {
        PermissionRequest {
            permission: format!("edit:{}", input.path.display()),
            reason: None,
            timeout: None,
        }
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        if input.find.is_empty() {
            return Err(ToolError::InvalidInput(
                "find string must not be empty".into(),
            ));
        }
        if input.find == input.replace {
            return Err(ToolError::InvalidInput(
                "find and replace are identical — no-op".into(),
            ));
        }
        if !ctx.mode.permits(PermissionMode::Danger)
            && !ctx.read_history.contains(&input.path).await
        {
            return Err(ToolError::ReadBeforeWriteRequired(
                input.path.display().to_string(),
            ));
        }

        let bytes = ctx.fs.read(&input.path, None).await?;
        let original = String::from_utf8(bytes.to_vec())
            .map_err(|_| ToolError::InvalidInput("file is not valid UTF-8".into()))?;

        let occurrences = original.matches(&input.find).count();
        if occurrences == 0 {
            return Err(ToolError::InvalidInput(format!(
                "find string not present in {}",
                input.path.display()
            )));
        }
        let new_content = if input.replace_all {
            original.replace(&input.find, &input.replace)
        } else {
            if occurrences > 1 {
                return Err(ToolError::InvalidInput(format!(
                    "find string matches {occurrences} locations — pass replace_all=true or add more context"
                )));
            }
            original.replacen(&input.find, &input.replace, 1)
        };

        let diff = crate::tools::diff::compute_line_diff(
            &original,
            &new_content,
            crate::tools::diff::DEFAULT_LINE_CAP,
        );

        let body = Bytes::from(new_content.into_bytes());
        let _meta = ctx
            .fs
            .write(&input.path, body, WriteOpts::default())
            .await?;

        Ok(EditOutput {
            path: input.path.display().to_string(),
            replacements: if input.replace_all { occurrences } else { 1 },
            diff,
        })
    }
}
