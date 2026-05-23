use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::error::ToolError;
use crate::types::permission::PermissionMode;
use crate::types::session::SessionId;

use super::event_sink::EventSink;
use super::permission_manager::PermissionManager;

/// Per-call context carrying handles a tool needs to enforce permissions
/// and emit events. Per amendment §B, ToolCtx already used `Arc<dyn _>`,
/// which is why moving AppState to dyn was free on the hot path.
#[derive(Clone)]
pub struct ToolCtx {
    pub session_id: SessionId,
    pub workspace_root: PathBuf,
    pub mode: PermissionMode,
    pub permission: Arc<dyn PermissionManager>,
    pub events: Arc<dyn EventSink>,
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
