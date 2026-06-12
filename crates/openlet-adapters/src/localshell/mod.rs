//! Local-shell adapter — implements `ShellExecutor` for the `bash` tool.
//!
//! Subprocess machinery: `tokio::process::Command::new("bash").arg("-lc")`,
//! `kill_on_drop(true)` always, output capped via `AsyncReadExt::take`,
//! timeout via `tokio::time::timeout` + explicit `child.kill()`. Env is
//! scrubbed to a small allowlist so subprocesses can't exfiltrate the
//! provider API key.

mod executor;
mod output_capture;

pub use executor::LocalShellExecutor;
