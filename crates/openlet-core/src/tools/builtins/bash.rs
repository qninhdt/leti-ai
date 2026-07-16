//! `bash` tool — runs commands via `LocalShellExecutor` adapter.
//!
//! The actual subprocess machinery lives in
//! `openlet-adapters/src/localshell/executor.rs`; this tool is a thin
//! `Tool`-trait wrapper that forwards to the executor we hold via
//! `Arc<dyn ShellExecutor>`. Permission string format: `bash:<command>`
//! so ruleset patterns like `bash:rm*` match literally.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::{CancellationPolicy, Tool};
use crate::types::permission::PermissionRequest;

/// Object-safe trait the runtime injects into `BashTool`. Phase 4B
/// implements this in `openlet-adapters` as `LocalShellExecutor`.
#[async_trait]
pub trait ShellExecutor: Send + Sync + 'static {
    async fn run(
        &self,
        ctx: &ToolCtx,
        command: &str,
        timeout_ms: u64,
    ) -> Result<BashOutput, ToolError>;
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct BashInput {
    pub command: String,
    /// Override the default 120_000 ms timeout. Capped at 600_000 ms.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Field-identical alias of [`crate::tools::builtins::ProcessOutput`].
pub type BashOutput = super::ProcessOutput;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;

#[derive(Default)]
pub struct BashTool {
    executor: Option<Arc<dyn ShellExecutor>>,
}

impl BashTool {
    #[must_use]
    pub fn with_executor(executor: Arc<dyn ShellExecutor>) -> Self {
        Self {
            executor: Some(executor),
        }
    }
}

#[async_trait]
impl Tool for BashTool {
    type Input = BashInput;
    type Output = BashOutput;

    fn name(&self) -> &'static str {
        "bash"
    }
    fn description(&self) -> &'static str {
        "Run a bash command. Supports pipes, redirects, globs, variables, \
         command substitution, and for/while/if. Also runs `python3`/`python` \
         (in-process; `-c CODE`, a script file, or piped stdin — no sys.argv). \
         Operates on the workspace filesystem; 120s default timeout, output \
         capped."
    }
    fn parallel_safe(&self) -> bool {
        false
    }
    fn cancellation_policy(&self) -> CancellationPolicy {
        CancellationPolicy::WaitForCleanup
    }

    fn permission(&self, input: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple(format!("bash:{}", input.command))
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        let executor = self
            .executor
            .as_ref()
            .ok_or_else(|| ToolError::Io("bash executor not configured".into()))?;
        let timeout_ms = input
            .timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);
        executor.run(&ctx, &input.command, timeout_ms).await
    }
}
