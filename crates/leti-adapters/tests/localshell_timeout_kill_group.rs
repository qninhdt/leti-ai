//! `LocalShellExecutor` SIGKILLs grandchildren on timeout.
//!
//! Without process-group kill, a malicious LLM could spawn
//! `(sleep 30 &)` to survive turn cancellation. The executor calls
//! `setpgid(0, 0)` on Unix so it can `killpg(SIGKILL)` the entire
//! group on timeout/cancel.
//!
//! Test strategy: run a bash command that backgrounds a child which
//! creates a sentinel file 5 s later. Set timeout 200 ms so the
//! parent dies fast; assert the sentinel never appears within 4 s.

#![cfg(unix)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use leti_adapters::localfs::LocalFilesystem;
use leti_adapters::localshell::LocalShellExecutor;
use leti_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use leti_core::adapters::event_sink::{EventSink, Persistence};
use leti_core::adapters::permission_manager::PermissionManager;
use leti_core::adapters::tool_executor::ToolCtx;
use leti_core::error::{ArtifactError, EventError, PermissionError};
use leti_core::permission::Deferred;
use leti_core::tools::ReadHistory;
use leti_core::tools::builtins::bash::ShellExecutor;
use leti_core::types::agent::AgentId;
use leti_core::types::event::{AgentEvent, EventFilter};
use leti_core::types::message::MessageId;
use leti_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionAction, PermissionCtx, PermissionMode,
    PermissionRequest, PermissionRule,
};
use leti_core::types::session::SessionId;
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
    fn take_deferred(&self, _: AskId) -> Option<Deferred<Decision>> {
        None
    }
    fn peek_session_id(&self, _: AskId) -> Option<SessionId> {
        None
    }
    async fn accept_ask(
        &self,
        _: AskId,
        _: AlwaysScope,
        _: PermissionAction,
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
    ) -> broadcast::Receiver<leti_core::adapters::event_sink::DeliveredEvent> {
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
        ext: Default::default(),
        session_id: SessionId::new(),
        agent_id: AgentId::new(),
        message_id: MessageId::new(),
        call_id: "call-pgroup".into(),
        fs: Arc::new(LocalFilesystem::new(workspace.to_path_buf())),
        mode: PermissionMode::Danger,
        permission: Arc::new(AllowAll),
        events: Arc::new(NoopBus),
        artifacts: Arc::new(DiscardArtifacts),
        read_history: ReadHistory::new(),
        cancel,
        questions: Arc::new(leti_core::runtime::QuestionRegistry::new()),
        memory: Arc::new(leti_adapters::sqlite::SqliteMemoryStore::new(
            // Use a closed pool — `record_read` is the only call site
            // and it isn't exercised by the tests below. Constructing
            // an in-memory pool would require an async tokio block at
            // ctx-build time which complicates the helper signature.
            sqlx::SqlitePool::connect_lazy("sqlite::memory:").expect("lazy pool"),
        )),
        task_registry: Arc::new(leti_core::runtime::subagent::TaskRegistry::new(32)),
        agent_registry: Arc::new(leti_core::agent::AgentRegistry::new()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "documents executor gap: parent-exits-cleanly leaves grandchildren alive (pgroup-kill only fires on timeout/cancel); production fix tracked separately"]
async fn timeout_kills_backgrounded_grandchild_when_parent_exits_cleanly() {
    let dir = TempDir::new().unwrap();
    let sentinel = dir.path().join("grandchild_alive");
    let sentinel_str = sentinel.to_string_lossy().to_string();

    let exec = LocalShellExecutor::new(dir.path().to_path_buf());

    // Parent backgrounds a grandchild then exits cleanly. Per the
    // current implementation in `executor.rs`, kill_group only fires
    // on the timeout/cancel branches — clean parent exit leaves the
    // grandchild alive. This test documents that gap; once the
    // executor reaps the full pgroup unconditionally on parent exit,
    // un-ignore this test.
    let cmd = format!("( ( sleep 4 && touch {sentinel_str} ) & ); echo spawned; exit 0");

    let _ = exec
        .run(&ctx(dir.path(), CancellationToken::new()), &cmd, 30_000)
        .await
        .expect("run");

    tokio::time::sleep(Duration::from_secs(5)).await;

    assert!(
        !sentinel.exists(),
        "grandchild touched sentinel at {sentinel_str} — process group not killed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn timeout_kills_grandchild_when_parent_blocks_past_budget() {
    // Verifies the documented contract: when the timeout fires, the
    // executor SIGKILLs the entire process group, taking out
    // backgrounded grandchildren. Parent doesn't exit before the
    // timeout, so the kill_group path is exercised.
    let dir = TempDir::new().unwrap();
    let sentinel = dir.path().join("grandchild_alive");
    let sentinel_str = sentinel.to_string_lossy().to_string();

    let exec = LocalShellExecutor::new(dir.path().to_path_buf());

    // Parent backgrounds a slow grandchild, then BLOCKS so the
    // executor's timeout fires. On timeout, kill_group(pgid) reaps
    // both parent and grandchild before the grandchild's sleep
    // completes.
    let cmd = format!("( sleep 5 && touch {sentinel_str} ) & sleep 30; wait");

    let out = exec
        .run(&ctx(dir.path(), CancellationToken::new()), &cmd, 300)
        .await
        .expect("run");
    assert!(out.timed_out, "expected timed_out=true");

    // Wait past when the grandchild's sleep would have finished.
    tokio::time::sleep(Duration::from_secs(6)).await;

    assert!(
        !sentinel.exists(),
        "grandchild survived timeout-triggered process-group kill"
    );
}
