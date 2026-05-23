//! `glob` tool — gitignore-aware filename matching via `ctx.fs.glob`.

use std::path::PathBuf;

use async_trait::async_trait;
use crate::adapters::filesystem::{GlobOpts, GlobSort};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::Tool;
use crate::types::permission::PermissionRequest;

const MAX_RESULTS: usize = 100;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GlobInput {
    /// Glob pattern (e.g. `src/**/*.rs`). Evaluated relative to the
    /// workspace root (per-`base` scoping was rarely useful and is
    /// dropped — narrow the pattern itself).
    pub pattern: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GlobOutput {
    pub matches: Vec<String>,
    pub truncated: bool,
}

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    type Input = GlobInput;
    type Output = GlobOutput;

    fn name(&self) -> &'static str {
        "glob"
    }
    fn description(&self) -> &'static str {
        "Find files by glob pattern. Honors .gitignore. Capped at 100 results, sorted by mtime desc."
    }
    fn parallel_safe(&self) -> bool {
        true
    }

    fn permission(&self, input: &Self::Input) -> PermissionRequest {
        PermissionRequest {
            permission: format!("read:glob:{}", input.pattern),
            reason: None,
            timeout: None,
        }
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        let opts = GlobOpts {
            respect_gitignore: true,
            max_results: MAX_RESULTS + 1,
            sort: GlobSort::MtimeDesc,
        };
        let mut hits = ctx.fs.glob(&input.pattern, opts).await?;
        let truncated = hits.len() > MAX_RESULTS;
        hits.truncate(MAX_RESULTS);
        let matches: Vec<String> = hits.into_iter().map(|p: PathBuf| p.display().to_string()).collect();
        Ok(GlobOutput { matches, truncated })
    }
}
