//! `LocalFilesystem` — `Filesystem` impl backed by `tokio::fs` +
//! `ignore::WalkBuilder`.
//!
//! Workspace boundary is enforced via deepest-existing-ancestor
//! canonicalize so writes/edits to non-existing targets
//! still resolve safely. Symlink TOCTOU is mitigated by lexical
//! normalize-then-canonicalize. Glob/grep honor `.gitignore` by
//! default (cloud impls fall back to their server-side index view —
//! we document the divergence in the trait, not here).

mod operations;
mod paths;
mod walk;

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use bytes::Bytes;
use openlet_core::adapters::filesystem::{
    ByteRange, DirEntry, FileMeta, Filesystem, GlobOpts, GrepArgs, GrepHit, WriteOpts,
};
use openlet_core::error::FsError;

#[derive(Debug, Clone)]
pub struct LocalFilesystem {
    root: PathBuf,
}

impl LocalFilesystem {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[async_trait]
impl Filesystem for LocalFilesystem {
    async fn read(&self, path: &Path, range: Option<ByteRange>) -> Result<Bytes, FsError> {
        operations::read(&self.root, path, range).await
    }

    async fn stat(&self, path: &Path) -> Result<FileMeta, FsError> {
        operations::stat(&self.root, path).await
    }

    async fn exists(&self, path: &Path) -> bool {
        operations::exists(&self.root, path).await
    }

    async fn write(&self, path: &Path, body: Bytes, opts: WriteOpts) -> Result<FileMeta, FsError> {
        operations::write(&self.root, path, body, opts).await
    }

    async fn remove(&self, path: &Path) -> Result<(), FsError> {
        operations::remove(&self.root, path).await
    }

    async fn rename(&self, from: &Path, to: &Path) -> Result<(), FsError> {
        operations::rename(&self.root, from, to).await
    }

    async fn list(&self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        operations::list(&self.root, path).await
    }

    async fn glob(&self, pattern: &str, opts: GlobOpts) -> Result<Vec<PathBuf>, FsError> {
        walk::glob(&self.root, pattern, opts).await
    }

    async fn grep(&self, args: GrepArgs) -> Result<Vec<GrepHit>, FsError> {
        walk::grep(&self.root, args).await
    }
}
