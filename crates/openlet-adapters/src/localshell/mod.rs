//! Local-shell `ToolExecutor` impl.
//!
//! Phase 1 stub. Phase 4 implements all six tools with workspace
//! canonicalization (§L) + permission checks (§E).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use bytes::Bytes;
use openlet_core::adapters::tool_executor::{
    BashCommand, BashOutput, DirEntry, FileBlob, GrepArgs, GrepHit, ToolCtx, ToolExecutor,
};
use openlet_core::error::ToolError;

#[derive(Debug, Default)]
pub struct LocalShellToolExecutor;

impl LocalShellToolExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ToolExecutor for LocalShellToolExecutor {
    async fn run_bash(
        &self,
        _ctx: ToolCtx,
        _cmd: BashCommand,
    ) -> Result<BashOutput, ToolError> {
        Err(ToolError::Unimplemented)
    }

    async fn read_file(
        &self,
        _ctx: ToolCtx,
        _path: &Path,
    ) -> Result<FileBlob, ToolError> {
        Err(ToolError::Unimplemented)
    }

    async fn write_file(
        &self,
        _ctx: ToolCtx,
        _path: &Path,
        _bytes: Bytes,
    ) -> Result<(), ToolError> {
        Err(ToolError::Unimplemented)
    }

    async fn list_dir(
        &self,
        _ctx: ToolCtx,
        _path: &Path,
    ) -> Result<Vec<DirEntry>, ToolError> {
        Err(ToolError::Unimplemented)
    }

    async fn glob(
        &self,
        _ctx: ToolCtx,
        _pattern: &str,
    ) -> Result<Vec<PathBuf>, ToolError> {
        Err(ToolError::Unimplemented)
    }

    async fn grep(
        &self,
        _ctx: ToolCtx,
        _args: GrepArgs,
    ) -> Result<Vec<GrepHit>, ToolError> {
        Err(ToolError::Unimplemented)
    }
}
