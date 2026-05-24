//! `LocalShellExecutor` integration — timeout, cancel, output cap, env
//! scrub. We exercise the executor directly (no `BashTool` wrapping) so
//! the failure modes show up as `BashOutput` flags / `ToolError`s.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use bytes::Bytes;
use openlet_adapters::localfs::LocalFilesystem;
use openlet_adapters::localshell::LocalShellExecutor;
use openlet_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use openlet_core::adapters::event_sink::{EventSink, Persistence};
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::error::{ArtifactError, EventError, PermissionError, ToolError};
use openlet_core::tools::ReadHistory;
use openlet_core::tools::builtins::bash::ShellExecutor;
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::{AgentEvent, EventFilter};
use openlet_core::types::message::MessageId;
use openlet_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionCtx, PermissionMode, PermissionRequest, PermissionRule,
};
use openlet_core::types::session::SessionId;
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct AllowAll;

#[async_trait]
impl PermissionManager for AllowAll {
    async fn check(
        &self,
        _: PermissionCtx,
        _: PermissionRequest,
    ) -> Result<Decision, PermissionError> {
        Ok(Decision::Allow)
    }
    async fn reply(&self, _: AskId, _: Decision) -> Result<(), PermissionError> {
        Ok(())
    }
    async fn cancel_ask(&self, _: AskId) -> Result<(), PermissionError> {
        Ok(())
    }
    async fn record_always(
        &self,
        _: AlwaysScope,
        _: PermissionRule,
    ) -> Result<(), PermissionError> {
        Ok(())
    }
}

#[derive(Default)]
struct NoopBus;

#[async_trait]
impl EventSink for NoopBus {
    async fn publish(&self, _: AgentEvent, _: Persistence) -> Result<(), EventError> {
        Ok(())
    }
    fn subscribe(
        &self,
        _: EventFilter,
    ) -> broadcast::Receiver<openlet_core::adapters::event_sink::DeliveredEvent> {
        let (_, rx) = broadcast::channel(1);
        rx
    }
}

#[derive(Default)]
struct DiscardArtifacts;

#[async_trait]
impl ArtifactStore for DiscardArtifacts {
    async fn put(
        &self,
        session: SessionId,
        key: &str,
        _: Bytes,
    ) -> Result<ArtifactRef, ArtifactError> {
        Ok(ArtifactRef {
            session_id: session,
            key: key.to_string(),
            size: 0,
            mime: None,
        })
    }
    async fn get(&self, _: &ArtifactRef) -> Result<Bytes, ArtifactError> {
        Err(ArtifactError::NotFound("test".into()))
    }
    async fn list(&self, _: SessionId) -> Result<Vec<ArtifactRef>, ArtifactError> {
        Ok(vec![])
    }
}

fn ctx(workspace: &Path, cancel: CancellationToken) -> ToolCtx {
    ToolCtx {
        session_id: SessionId::new(),
        agent_id: AgentId::new(),
        message_id: MessageId::new(),
        call_id: "call-bash-test".into(),
        fs: Arc::new(LocalFilesystem::new(workspace.to_path_buf())),
        mode: PermissionMode::Danger,
        permission: Arc::new(AllowAll),
        events: Arc::new(NoopBus),
        artifacts: Arc::new(DiscardArtifacts),
        read_history: ReadHistory::new(),
        cancel,
    }
}

#[tokio::test]
async fn echo_runs_in_workspace() {
    let dir = TempDir::new().unwrap();
    let exec = LocalShellExecutor::new(dir.path().to_path_buf());
    let out = exec
        .run(
            &ctx(dir.path(), CancellationToken::new()),
            "echo hi && pwd",
            5_000,
        )
        .await
        .unwrap();
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("hi"));
    // pwd should report the canonicalized workspace dir.
    let canon = dir.path().canonicalize().unwrap();
    assert!(out.stdout.contains(canon.to_str().unwrap()));
    assert!(!out.timed_out);
}

#[tokio::test]
async fn timeout_kills_long_running_child() {
    let dir = TempDir::new().unwrap();
    let exec = LocalShellExecutor::new(dir.path().to_path_buf());
    let started = Instant::now();
    let out = exec
        .run(&ctx(dir.path(), CancellationToken::new()), "sleep 10", 200)
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert!(out.timed_out, "expected timed_out=true, got {out:?}");
    assert!(
        elapsed < Duration::from_secs(2),
        "took too long: {elapsed:?}"
    );
    // exit code is -1 sentinel for timeout.
    assert_eq!(out.exit_code, -1);
}

#[tokio::test]
async fn cancel_token_aborts_child() {
    let dir = TempDir::new().unwrap();
    let exec = LocalShellExecutor::new(dir.path().to_path_buf());
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(80)).await;
        cancel_clone.cancel();
    });
    let started = Instant::now();
    let res = exec.run(&ctx(dir.path(), cancel), "sleep 30", 30_000).await;
    let elapsed = started.elapsed();
    assert!(matches!(res, Err(ToolError::Timeout)), "got {res:?}");
    assert!(
        elapsed < Duration::from_secs(2),
        "took too long: {elapsed:?}"
    );
}

#[tokio::test]
async fn stdout_caps_at_256_kib() {
    let dir = TempDir::new().unwrap();
    let exec = LocalShellExecutor::new(dir.path().to_path_buf());
    // Print 1 MiB of zeros — way past the 256 KiB cap.
    let out = exec
        .run(
            &ctx(dir.path(), CancellationToken::new()),
            "head -c 1048576 /dev/zero | base64 -w0",
            10_000,
        )
        .await
        .unwrap();
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout_truncated, "stdout should be truncated past cap");
    assert!(
        out.stdout.len() <= 256 * 1024,
        "stdout length {} exceeds cap",
        out.stdout.len()
    );
}

#[tokio::test]
async fn stderr_caps_at_64_kib() {
    let dir = TempDir::new().unwrap();
    let exec = LocalShellExecutor::new(dir.path().to_path_buf());
    let out = exec
        .run(
            &ctx(dir.path(), CancellationToken::new()),
            "head -c 262144 /dev/zero | base64 -w0 1>&2",
            10_000,
        )
        .await
        .unwrap();
    assert_eq!(out.exit_code, 0);
    assert!(out.stderr_truncated, "stderr should be truncated past cap");
    assert!(out.stderr.len() <= 64 * 1024);
}

#[tokio::test]
async fn env_scrub_strips_disallowed_vars() {
    // `CARGO` is set by cargo when running tests but is NOT in the
    // executor's allowlist, so it must be stripped inside the subshell.
    // Avoids needing `unsafe { std::env::set_var(..) }` (which the
    // workspace forbids via `-F unsafe-code`).
    assert!(
        std::env::var("CARGO").is_ok(),
        "CARGO must be set in the cargo test env for this assertion to be meaningful"
    );

    let dir = TempDir::new().unwrap();
    let exec = LocalShellExecutor::new(dir.path().to_path_buf());
    let out = exec
        .run(
            &ctx(dir.path(), CancellationToken::new()),
            "echo \"cargo=${CARGO:-MISSING}\"",
            5_000,
        )
        .await
        .unwrap();
    assert_eq!(out.exit_code, 0);
    assert!(
        out.stdout.contains("cargo=MISSING"),
        "expected CARGO scrubbed, got stdout={:?}",
        out.stdout
    );

    // PATH must still be present so coreutils resolve.
    let out2 = exec
        .run(
            &ctx(dir.path(), CancellationToken::new()),
            "echo \"path=${PATH:-MISSING}\"",
            5_000,
        )
        .await
        .unwrap();
    assert!(
        !out2.stdout.contains("path=MISSING"),
        "PATH should pass through"
    );
}

#[tokio::test]
async fn nonzero_exit_is_returned_not_errored() {
    let dir = TempDir::new().unwrap();
    let exec = LocalShellExecutor::new(dir.path().to_path_buf());
    let out = exec
        .run(&ctx(dir.path(), CancellationToken::new()), "exit 7", 5_000)
        .await
        .unwrap();
    assert_eq!(out.exit_code, 7);
    assert!(!out.timed_out);
}
