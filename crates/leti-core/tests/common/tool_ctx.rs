//! Minimal `ToolCtx` builder for tool-surface unit tests.
//!
//! Wires the in-memory mocks (`AllowAll`, `RecordingEventSink`,
//! `MemArtifactStore`, `MockMemoryStore`) + a `LocalFilesystem` rooted at a
//! throwaway tempdir so a tool's `run` can be exercised without booting a
//! runtime or `AppState`. The tempdir is leaked into the `ToolCtx` (kept
//! alive for the process) — tests here don't touch the filesystem.

use std::sync::Arc;

use leti_adapters::localfs::LocalFilesystem;
use leti_core::adapters::tool_executor::ToolCtx;
use leti_core::agent::AgentRegistry;
use leti_core::runtime::question_registry::QuestionRegistry;
use leti_core::runtime::subagent::TaskRegistry;
use leti_core::tools::ReadHistory;
use leti_core::types::agent::AgentId;
use leti_core::types::message::MessageId;
use leti_core::types::permission::PermissionMode;
use leti_core::types::session::SessionId;
use tokio_util::sync::CancellationToken;

use super::mock_artifact::MemArtifactStore;
use super::mock_event_sink::RecordingEventSink;
use super::mock_memory::MockMemoryStore;
use super::mock_permission::AllowAll;

/// Build a `ToolCtx` backed entirely by in-memory mocks. Every handle is
/// a fresh instance; the workspace filesystem is rooted at a leaked
/// tempdir so file tools resolve but tests here never write.
#[must_use]
pub fn minimal_tool_ctx() -> ToolCtx {
    minimal_tool_ctx_with_registry(Arc::new(TaskRegistry::new(32)))
}

/// Build a `ToolCtx` for a SPECIFIC sender session, sharing a caller-owned
/// memory store, task registry, and agent registry. Used by `send_message`
/// tests that need the tool's session walk (`ctx.memory.get_session`) +
/// allowlist resolution (`ctx.agent_registry`) to see seeded parent/child
/// session metas and agent defs.
#[must_use]
pub fn tool_ctx_with(
    session_id: SessionId,
    memory: Arc<dyn leti_core::adapters::memory_store::MemoryStore>,
    task_registry: Arc<TaskRegistry>,
    agent_registry: Arc<AgentRegistry>,
) -> ToolCtx {
    let workspace = tempfile::tempdir().expect("tempdir").keep();
    ToolCtx {
        session_id,
        agent_id: AgentId::new(),
        message_id: MessageId::new(),
        call_id: "call-test".into(),
        ext: Default::default(),
        mode: PermissionMode::Danger,
        fs: Arc::new(LocalFilesystem::new(workspace)),
        permission: Arc::new(AllowAll),
        events: Arc::new(RecordingEventSink::new()),
        artifacts: Arc::new(MemArtifactStore::new()),
        read_history: ReadHistory::new(),
        cancel: CancellationToken::new(),
        questions: Arc::new(QuestionRegistry::new()),
        memory,
        task_registry,
        agent_registry,
    }
}

/// Like [`minimal_tool_ctx`] but with a caller-supplied task registry, so
/// a test can install tasks and then exercise a tool,
/// `task_status`) that reads the SAME registry via `ctx.task_registry`.
#[must_use]
pub fn minimal_tool_ctx_with_registry(task_registry: Arc<TaskRegistry>) -> ToolCtx {
    let workspace = tempfile::tempdir().expect("tempdir").keep();
    ToolCtx {
        session_id: SessionId::new(),
        agent_id: AgentId::new(),
        message_id: MessageId::new(),
        call_id: "call-test".into(),
        ext: Default::default(),
        mode: PermissionMode::Danger,
        fs: Arc::new(LocalFilesystem::new(workspace)),
        permission: Arc::new(AllowAll),
        events: Arc::new(RecordingEventSink::new()),
        artifacts: Arc::new(MemArtifactStore::new()),
        read_history: ReadHistory::new(),
        cancel: CancellationToken::new(),
        questions: Arc::new(QuestionRegistry::new()),
        memory: Arc::new(MockMemoryStore::new()),
        task_registry,
        agent_registry: Arc::new(AgentRegistry::new()),
    }
}
