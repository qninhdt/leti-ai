//! `grep` tool — line-anchored regex search via `ctx.fs.grep`.

use crate::adapters::filesystem::GrepArgs as FsGrepArgs;
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::Tool;
use crate::types::permission::PermissionRequest;

const MAX_HITS: usize = 250;
const MAX_LINE_LENGTH: usize = 2000;

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GrepInput {
    pub pattern: String,
    /// Optional path glob (e.g. `**/*.rs`) to scope the walk.
    #[serde(default)]
    pub path_glob: Option<String>,
    #[serde(default)]
    pub case_insensitive: bool,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GrepHit {
    pub path: String,
    pub line: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct GrepOutput {
    pub hits: Vec<GrepHit>,
    pub truncated: bool,
}

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    type Input = GrepInput;
    type Output = GrepOutput;

    fn name(&self) -> &'static str {
        "grep"
    }
    fn description(&self) -> &'static str {
        "Search files by regex. Honors .gitignore. Capped at 250 hits, 2000 chars per line."
    }
    fn parallel_safe(&self) -> bool {
        true
    }

    fn permission(&self, input: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple(format!("read:grep:{}", input.pattern))
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        let raw = ctx
            .fs
            .grep(FsGrepArgs {
                pattern: input.pattern,
                path_glob: input.path_glob,
                case_insensitive: input.case_insensitive,
                max_hits: MAX_HITS + 1,
                max_line_chars: MAX_LINE_LENGTH,
            })
            .await?;
        let truncated = raw.len() > MAX_HITS;
        let hits: Vec<GrepHit> = raw
            .into_iter()
            .take(MAX_HITS)
            .map(|h| GrepHit {
                path: h.path.display().to_string(),
                line: h.line,
                text: h.text,
            })
            .collect();
        Ok(GrepOutput { hits, truncated })
    }
}
