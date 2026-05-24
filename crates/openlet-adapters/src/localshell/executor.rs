//! `LocalShellExecutor` — runs bash commands via `tokio::process` with
//! kill_on_drop, capped output streams, env scrub, and timeout.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::error::ToolError;
use openlet_core::tools::builtins::bash::{BashOutput, ShellExecutor};
use tokio::process::Command;
use tokio::time::timeout;

use super::output_capture::read_capped;

const MAX_STDOUT: usize = 256 * 1024;
const MAX_STDERR: usize = 64 * 1024;

/// Env vars passed to subprocesses. Anything else is dropped so a
/// command can't leak `OPENROUTER_API_KEY` or similar via `env`.
const ENV_ALLOWLIST: &[&str] = &[
    "PATH", "HOME", "USER", "LANG", "LC_ALL", "LC_CTYPE", "TERM", "TZ", "SHELL", "TMPDIR",
];

#[derive(Debug, Clone)]
pub struct LocalShellExecutor {
    workspace_root: PathBuf,
}

impl LocalShellExecutor {
    #[must_use]
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }
}

#[async_trait]
impl ShellExecutor for LocalShellExecutor {
    async fn run(
        &self,
        ctx: &ToolCtx,
        command: &str,
        timeout_ms: u64,
    ) -> Result<BashOutput, ToolError> {
        let mut cmd = Command::new("bash");
        cmd.arg("-lc")
            .arg(command)
            .current_dir(&self.workspace_root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        cmd.env_clear();
        let scrubbed = scrubbed_env();
        for (k, v) in &scrubbed {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| ToolError::Io(format!("spawn bash: {e}")))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| ToolError::Io("missing stdout pipe".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| ToolError::Io("missing stderr pipe".into()))?;

        let cancel = ctx.cancel.clone();
        let stdout_handle = tokio::spawn(async move { read_capped(&mut stdout, MAX_STDOUT).await });
        let stderr_handle = tokio::spawn(async move { read_capped(&mut stderr, MAX_STDERR).await });

        let dur = Duration::from_millis(timeout_ms);
        let exit_result = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(ToolError::Timeout);
            }
            res = timeout(dur, child.wait()) => res,
        };

        let timed_out = exit_result.is_err();
        if timed_out {
            let _ = child.start_kill();
        }
        let exit_status = match exit_result {
            Ok(Ok(s)) => Some(s),
            Ok(Err(e)) => return Err(ToolError::Io(format!("wait: {e}"))),
            Err(_) => None,
        };

        let (stdout_bytes, stdout_truncated) = stdout_handle
            .await
            .map_err(|e| ToolError::Io(format!("stdout join: {e}")))?
            .map_err(|e| ToolError::Io(format!("stdout read: {e}")))?;
        let (stderr_bytes, stderr_truncated) = stderr_handle
            .await
            .map_err(|e| ToolError::Io(format!("stderr join: {e}")))?
            .map_err(|e| ToolError::Io(format!("stderr read: {e}")))?;

        let exit_code =
            exit_status
                .and_then(|s| s.code())
                .unwrap_or(if timed_out { -1 } else { -2 });

        Ok(BashOutput {
            stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
            exit_code,
            timed_out,
            stdout_truncated,
            stderr_truncated,
        })
    }
}

fn scrubbed_env() -> HashMap<String, String> {
    let mut out = HashMap::new();
    for key in ENV_ALLOWLIST {
        if let Ok(v) = std::env::var(key) {
            out.insert((*key).to_string(), v);
        }
    }
    if !out.contains_key("PATH") {
        out.insert("PATH".to_string(), "/usr/bin:/bin".to_string());
    }
    out
}
