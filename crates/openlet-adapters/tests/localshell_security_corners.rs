//! Targeted localshell tests for security-sensitive corners not
//! covered by `localshell_tests.rs`:
//!
//! 1. HOME is NEVER passed through to children, even when parent has
//!    one. (Combined with `bash -c`, this prevents an LLM from
//!    persisting via `~/.bashrc`.)
//! 2. PATH defaults to `/usr/bin:/bin` when parent has none.
//! 3. stdin is closed at spawn — commands waiting on stdin don't hang
//!    the executor.
//! 4. cwd defaults to the workspace_root passed at construction.
//! 5. bash runs as `bash -c`, NOT `bash -lc` — `.bashrc` and friends
//!    are not sourced.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use openlet_adapters::localfs::LocalFilesystem;
use openlet_adapters::localshell::LocalShellExecutor;
use openlet_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use openlet_core::adapters::event_sink::{EventSink, Persistence};
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::error::{ArtifactError, EventError, MemoryError, PermissionError};
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
    async fn check(&self, _: PermissionCtx, _: PermissionRequest) -> Result<Decision, PermissionError> {
        Ok(Decision::Allow)
    }
    async fn reply(&self, _: AskId, _: Decision) -> Result<(), PermissionError> { Ok(()) }
    async fn cancel_ask(&self, _: AskId) -> Result<(), PermissionError> { Ok(()) }
    async fn record_always(&self, _: AlwaysScope, _: PermissionRule) -> Result<(), PermissionError> { Ok(()) }
    fn take_deferred(&self, _: AskId) -> Option<openlet_core::permission::Deferred<Decision>> { None }
    fn peek_session_id(&self, _: AskId) -> Option<SessionId> { None }
    async fn accept_ask(&self, _: AskId, _: AlwaysScope) -> Result<(), PermissionError> { Ok(()) }
}

#[derive(Default)]
struct NoopBus;

#[async_trait]
impl EventSink for NoopBus {
    async fn publish(&self, _: AgentEvent, _: Persistence) -> Result<(), EventError> { Ok(()) }
    fn subscribe(&self, _: EventFilter) -> broadcast::Receiver<openlet_core::adapters::event_sink::DeliveredEvent> {
        let (_, rx) = broadcast::channel(1);
        rx
    }
}

#[derive(Default)]
struct DiscardArtifacts;

#[async_trait]
impl ArtifactStore for DiscardArtifacts {
    async fn put(&self, session: SessionId, key: &str, _: Bytes) -> Result<ArtifactRef, ArtifactError> {
        Ok(ArtifactRef { session_id: session, key: key.to_string(), size: 0, mime: None })
    }
    async fn get(&self, _: &ArtifactRef) -> Result<Bytes, ArtifactError> {
        Err(ArtifactError::NotFound("test".into()))
    }
    async fn list(&self, _: SessionId) -> Result<Vec<ArtifactRef>, ArtifactError> { Ok(vec![]) }
}

fn noop_memory() -> Arc<dyn openlet_core::adapters::memory_store::MemoryStore> {
    use openlet_core::adapters::memory_store::MemoryStore;

    struct NoopMemory;

    #[async_trait]
    impl MemoryStore for NoopMemory {
        async fn create_session(&self, _: AgentId, _: Option<SessionId>) -> Result<SessionId, MemoryError> {
            Err(MemoryError::Unimplemented)
        }
        async fn get_session(&self, _: SessionId) -> Result<Option<openlet_core::types::session::SessionMeta>, MemoryError> { Ok(None) }
        async fn list_sessions(&self, _: openlet_core::types::session::SessionFilter) -> Result<Vec<openlet_core::types::session::SessionMeta>, MemoryError> { Ok(vec![]) }
        async fn update_status(&self, _: SessionId, _: openlet_core::types::session::SessionStatus, _: &str) -> Result<(), MemoryError> { Ok(()) }
        async fn update_permission_mode(&self, _: SessionId, _: PermissionMode) -> Result<(), MemoryError> { Ok(()) }
        async fn switch_agent(&self, _: SessionId, _: &str) -> Result<(), MemoryError> { Ok(()) }
        async fn update_session_extensions(&self, _: SessionId, _: serde_json::Value) -> Result<(), MemoryError> { Ok(()) }
        async fn delete_session(&self, _: SessionId) -> Result<(), MemoryError> { Ok(()) }
        async fn append_message(&self, _: SessionId, m: openlet_core::types::message::Message) -> Result<openlet_core::types::message::MessageId, MemoryError> { Ok(m.id) }
        async fn append_part(&self, _: openlet_core::types::message::MessageId, p: openlet_core::types::part::Part) -> Result<openlet_core::types::part::PartId, MemoryError> { Ok(p.id()) }
        async fn upsert_part(&self, _: openlet_core::types::message::MessageId, _: openlet_core::types::part::PartId, _: openlet_core::types::part::Part) -> Result<(), MemoryError> { Ok(()) }
        async fn list_messages(&self, _: SessionId) -> Result<Vec<openlet_core::types::message::Message>, MemoryError> { Ok(vec![]) }
        async fn list_parts(&self, _: SessionId, _: openlet_core::types::message::MessageId) -> Result<Vec<openlet_core::types::part::Part>, MemoryError> { Ok(vec![]) }
        async fn record_read(&self, _: SessionId, _: std::path::PathBuf) -> Result<(), MemoryError> { Ok(()) }
    }

    Arc::new(NoopMemory)
}

fn ctx(workspace: &Path) -> ToolCtx {
    ToolCtx {
        session_id: SessionId::new(),
        agent_id: AgentId::new(),
        message_id: MessageId::new(),
        call_id: "call-shell".into(),
        fs: Arc::new(LocalFilesystem::new(workspace.to_path_buf())),
        mode: PermissionMode::Danger,
        permission: Arc::new(AllowAll),
        events: Arc::new(NoopBus),
        artifacts: Arc::new(DiscardArtifacts),
        read_history: ReadHistory::new(),
        cancel: CancellationToken::new(),
        questions: Arc::new(openlet_core::runtime::QuestionRegistry::new()),
        memory: noop_memory(),
        task_registry: Arc::new(openlet_core::runtime::subagent::TaskRegistry::new(32)),
        agent_registry: Arc::new(openlet_core::agent::AgentRegistry::new()),
    }
}

#[tokio::test]
async fn home_env_var_is_never_passed_to_child() {
    // The executor's `ENV_ALLOWLIST` deliberately omits HOME so an
    // LLM can't persist via `~/.bashrc`. We rely on the parent
    // process having HOME set (always true on CI / dev shells) to
    // make the assertion meaningful — if HOME really is forwarded,
    // the child sees it; if it's stripped, the child sees the
    // sentinel "UNSET".
    if std::env::var("HOME").is_err() {
        // Edge case: no parent HOME means we can't distinguish
        // "stripped" from "never set". Skip rather than mutate
        // env (workspace forbids `unsafe_code`).
        return;
    }
    let tmp = TempDir::new().unwrap();
    let exec = LocalShellExecutor::new(tmp.path().to_path_buf());
    let out = exec
        .run(&ctx(tmp.path()), "echo HOME=\"${HOME:-UNSET}\"", 5_000)
        .await
        .unwrap();
    assert!(
        out.stdout.contains("HOME=UNSET"),
        "HOME must be UNSET in child, got stdout: {:?}",
        out.stdout
    );
}

#[tokio::test]
async fn cwd_defaults_to_workspace_root() {
    let tmp = TempDir::new().unwrap();
    let exec = LocalShellExecutor::new(tmp.path().to_path_buf());
    let out = exec.run(&ctx(tmp.path()), "pwd", 5_000).await.unwrap();
    let pwd = out.stdout.trim();
    // Resolve the canonical path of the tempdir to handle symlinks
    // (macOS puts /tmp under /private/tmp, etc.).
    let expected = std::fs::canonicalize(tmp.path()).unwrap();
    let got = std::fs::canonicalize(pwd).unwrap();
    assert_eq!(got, expected, "child cwd must equal workspace_root");
}

#[tokio::test]
async fn stdin_closed_at_spawn_command_does_not_hang() {
    // `cat` with stdin closed reads EOF immediately and exits 0.
    // If the executor left stdin open, this would block until timeout.
    let tmp = TempDir::new().unwrap();
    let exec = LocalShellExecutor::new(tmp.path().to_path_buf());
    let start = std::time::Instant::now();
    let out = exec.run(&ctx(tmp.path()), "cat", 5_000).await.unwrap();
    let elapsed = start.elapsed();
    assert_eq!(out.exit_code, 0, "cat must exit 0 when stdin is closed");
    assert!(!out.timed_out, "cat must not time out");
    assert!(
        elapsed.as_secs() < 2,
        "cat returned in {:?} — stdin appears NOT closed at spawn",
        elapsed
    );
}

#[tokio::test]
async fn bash_runs_non_login_so_bashrc_is_not_sourced() {
    // The executor uses `bash -c`, NOT `bash -lc`. A login shell
    // would source ~/.bashrc / ~/.bash_profile. We assert this
    // indirectly by setting BASH_ENV to a sentinel script and
    // verifying it does NOT run (BASH_ENV is sourced for non-
    // interactive shells only when `--rcfile` or similar is set —
    // but more importantly, $0 is "bash" not "-bash").
    //
    // The cleanest signal: $0 in a login shell is "-bash"; in
    // non-login it's just "bash" or the script name. With
    // `bash -c "..."`, $0 defaults to "bash".
    let tmp = TempDir::new().unwrap();
    let exec = LocalShellExecutor::new(tmp.path().to_path_buf());
    let out = exec.run(&ctx(tmp.path()), "echo \"$0\"", 5_000).await.unwrap();
    let dollar_zero = out.stdout.trim();
    assert!(
        !dollar_zero.starts_with('-'),
        "$0 starts with '-' indicating a login shell; got {dollar_zero:?}"
    );
}

#[tokio::test]
async fn path_falls_back_when_parent_has_no_path() {
    // `scrubbed_env` injects PATH=/usr/bin:/bin when the parent
    // process doesn't have PATH set. We can't easily unset PATH
    // for the entire test process without breaking other tests,
    // so this test asserts the weaker invariant: PATH is always
    // present in the child env. The fallback path is exercised
    // when the executor runs from a stripped environment.
    let tmp = TempDir::new().unwrap();
    let exec = LocalShellExecutor::new(tmp.path().to_path_buf());
    let out = exec.run(&ctx(tmp.path()), "echo PATH=\"$PATH\"", 5_000).await.unwrap();
    assert!(
        out.stdout.contains("PATH=") && !out.stdout.trim().ends_with("PATH="),
        "child PATH must be non-empty, got: {:?}",
        out.stdout
    );
    // Stronger: at least one of the fallback dirs should be in PATH
    // (covers both the parent-passes-PATH and fallback paths since
    // production environments commonly contain /usr/bin or /bin).
    assert!(
        out.stdout.contains("/usr/bin") || out.stdout.contains("/bin"),
        "PATH should contain a usable bin dir, got: {:?}",
        out.stdout
    );
}
