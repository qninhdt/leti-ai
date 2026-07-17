//! `EmulatedShellExecutor` — the `ShellExecutor` impl backed by the
//! in-process bash interpreter instead of a real subprocess.
//!
//! It holds no workspace root of its own: the workspace lives behind
//! `ctx.fs`, so the same executor runs identically against the local FS or
//! a cloud gRPC backend. There is no process, no env, no network — the
//! interpreter physically cannot reach any of them.

use std::time::Duration;

use async_trait::async_trait;
use leti_core::adapters::tool_executor::ToolCtx;
use leti_core::error::ToolError;
use leti_core::tools::builtins::bash::{BashOutput, ShellExecutor};

use super::eval::{AbortReason, Interp};
use super::parse::parse;

/// Stateless executor — everything it needs comes from the per-call
/// `ToolCtx` (the FS seam and the cancel token).
#[derive(Debug, Clone, Default)]
pub struct EmulatedShellExecutor;

impl EmulatedShellExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ShellExecutor for EmulatedShellExecutor {
    async fn run(
        &self,
        ctx: &ToolCtx,
        command: &str,
        timeout_ms: u64,
    ) -> Result<BashOutput, ToolError> {
        // Parse failures are an interpreter-infrastructure error, not a
        // command failure — but we still surface them as a non-zero exit
        // with the parse message on stderr so the LLM can react, rather
        // than crashing the tool call. Only a genuinely unusable grammar
        // load returns `Err(ToolError)`.
        let script = match parse(command) {
            Ok(s) => s,
            Err(e) => {
                return Ok(BashOutput {
                    stdout: String::new(),
                    stderr: format!("bash: parse error: {e}\n"),
                    exit_code: 2,
                    timed_out: false,
                    stdout_truncated: false,
                    stderr_truncated: false,
                });
            }
        };

        // The interpreter enforces the wall-clock cutoff in-band (checked from
        // `tick`), so a pure-CPU `while true` loop that never `.await`s the
        // filesystem is still bounded — a wrapping `tokio::time::timeout`
        // alone cannot pre-empt such a loop because it never yields. The step
        // budget is a second backstop below that.
        let interp = Interp::new(ctx.fs.as_ref(), &ctx.cancel)
            .with_timeout(Duration::from_millis(timeout_ms));
        let mut result = interp.run(&script).await;

        // A wall-clock timeout or step-budget exhaustion both map to
        // `timed_out` so the runtime treats them like the old subprocess
        // timeout path. Append a distinct stderr line so the LLM (and logs)
        // can tell WHICH guard fired.
        let timed_out = matches!(
            result.aborted,
            Some(AbortReason::Timeout) | Some(AbortReason::StepBudget)
        );
        match result.aborted {
            Some(AbortReason::Timeout) => {
                result.stderr.push_str(&format!(
                    "bash: terminated: wall-clock timeout ({timeout_ms}ms)\n"
                ));
            }
            Some(AbortReason::StepBudget) => {
                result
                    .stderr
                    .push_str("bash: terminated: step budget exceeded\n");
            }
            _ => {}
        }

        // Cancellation (not a resource guard) is the runtime asking us to
        // stop — the old executor returned `Err(ToolError::Timeout)` for that,
        // so keep the contract for cancel specifically.
        if matches!(result.aborted, Some(AbortReason::Cancelled)) {
            return Err(ToolError::Timeout);
        }

        Ok(BashOutput {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: if timed_out { -1 } else { result.exit_code },
            timed_out,
            stdout_truncated: result.stdout_truncated,
            stderr_truncated: result.stderr_truncated,
        })
    }
}
