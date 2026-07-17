//! `python` tool — runs Python code via a `PythonExecutor` adapter.
//!
//! The actual interpreter lives in `leti-adapters/src/pyexec/`
//! (a Monty VM driven through the `Filesystem` seam); this tool is a thin
//! `Tool`-trait wrapper that forwards to the executor we hold via
//! `Arc<dyn PythonExecutor>`. Permission string format: `python:<code>`
//! so ruleset patterns match the code literally.
//!
//! Like the `bash` tool, this is *security by construction*: the executor
//! has no branch that spawns a process, opens a socket, or touches the host
//! filesystem — every IO hop routes through the injected `Filesystem`.

use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::tools::{CancellationPolicy, Tool};
use crate::types::permission::PermissionRequest;

/// Object-safe trait the runtime injects into `PythonTool`. Implemented in
/// `leti-adapters` as `MontyExecutor`.
#[async_trait]
pub trait PythonExecutor: Send + Sync + 'static {
    async fn run(
        &self,
        ctx: &ToolCtx,
        code: &str,
        timeout_ms: u64,
    ) -> Result<PythonOutput, ToolError>;
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PythonInput {
    /// Python source to execute. The module's last expression is echoed to
    /// stdout (REPL-style) in addition to anything `print`ed.
    pub code: String,
    /// Override the default 30_000 ms timeout. Capped at 120_000 ms.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Field-identical alias of [`crate::tools::builtins::ProcessOutput`].
pub type PythonOutput = super::ProcessOutput;

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_TIMEOUT_MS: u64 = 120_000;

#[derive(Default)]
pub struct PythonTool {
    executor: Option<Arc<dyn PythonExecutor>>,
}

impl PythonTool {
    #[must_use]
    pub fn with_executor(executor: Arc<dyn PythonExecutor>) -> Self {
        Self {
            executor: Some(executor),
        }
    }
}

#[async_trait]
impl Tool for PythonTool {
    type Input = PythonInput;
    type Output = PythonOutput;

    fn name(&self) -> &'static str {
        "python"
    }
    fn description(&self) -> &'static str {
        "Run Python code. Supports arithmetic, strings, json, re, \
         comprehensions, functions, and file IO via open()/pathlib. Operates \
         on the workspace filesystem; 30s default timeout, output capped."
    }
    fn parallel_safe(&self) -> bool {
        false
    }
    fn cancellation_policy(&self) -> CancellationPolicy {
        CancellationPolicy::WaitForCleanup
    }

    fn permission(&self, input: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple(format!("python:{}", input.code))
    }

    async fn run(&self, ctx: ToolCtx, input: Self::Input) -> Result<Self::Output, ToolError> {
        let executor = self
            .executor
            .as_ref()
            .ok_or_else(|| ToolError::Io("python executor not configured".into()))?;
        let timeout_ms = input
            .timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);
        executor.run(&ctx, &input.code, timeout_ms).await
    }
}
