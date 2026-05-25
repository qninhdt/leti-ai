use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::ToolError;
use crate::runtime::question_registry::QuestionRegistry;
use crate::tools::read_history::ReadHistory;
use crate::types::agent::AgentId;
use crate::types::message::MessageId;
use crate::types::permission::PermissionMode;
use crate::types::session::SessionId;

use super::artifact_store::ArtifactStore;
use super::event_sink::EventSink;
use super::filesystem::Filesystem;
use super::memory_store::MemoryStore;
use super::permission_manager::PermissionManager;

/// Per-call context carrying handles a tool needs to enforce permissions
/// and emit events. Per amendment §B, ToolCtx already used `Arc<dyn _>`,
/// which is why moving AppState to dyn was free on the hot path. The
/// filesystem is itself an `Arc<dyn Filesystem>` (Phase 4D) — built-in
/// file tools (`read`/`write`/`edit`/`list`/`glob`/`grep`) call
/// `ctx.fs.*` so a cloud impl can swap the workspace backing without
/// touching tool code.
#[derive(Clone)]
pub struct ToolCtx {
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub message_id: MessageId,
    pub call_id: String,
    pub mode: PermissionMode,
    pub fs: Arc<dyn Filesystem>,
    pub permission: Arc<dyn PermissionManager>,
    pub events: Arc<dyn EventSink>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub read_history: ReadHistory,
    pub cancel: CancellationToken,
    /// In-flight `ask_user` rendezvous map. The `ask_user` tool registers
    /// a oneshot here at run-time; the REST handler resolves the entry on
    /// the matching `POST /v1/sessions/:id/question/answer` reply.
    pub questions: Arc<QuestionRegistry>,
    /// Memory-store handle. Tools that need to inspect session-level
    /// state (capabilities, extensions, permission mode) read through
    /// this — kept on `ToolCtx` so the runtime stays the only authority
    /// on which adapter implementation backs the lookup.
    pub memory: Arc<dyn MemoryStore>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashCommand {
    pub command: String,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BashOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileBlob {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub line_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepArgs {
    pub pattern: String,
    pub path: Option<PathBuf>,
    pub case_insensitive: bool,
    pub regex: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrepHit {
    pub path: PathBuf,
    pub line: u64,
    pub text: String,
}

/// Six built-in tool methods. Phase 4 implements `LocalShellToolExecutor`
/// with workspace canonicalization (§L).
#[async_trait]
pub trait ToolExecutor: Send + Sync + 'static {
    async fn run_bash(&self, ctx: ToolCtx, cmd: BashCommand) -> Result<BashOutput, ToolError>;
    async fn read_file(&self, ctx: ToolCtx, path: &Path) -> Result<FileBlob, ToolError>;
    async fn write_file(&self, ctx: ToolCtx, path: &Path, bytes: Bytes) -> Result<(), ToolError>;
    async fn list_dir(&self, ctx: ToolCtx, path: &Path) -> Result<Vec<DirEntry>, ToolError>;
    async fn glob(&self, ctx: ToolCtx, pattern: &str) -> Result<Vec<PathBuf>, ToolError>;
    async fn grep(&self, ctx: ToolCtx, args: GrepArgs) -> Result<Vec<GrepHit>, ToolError>;
}
