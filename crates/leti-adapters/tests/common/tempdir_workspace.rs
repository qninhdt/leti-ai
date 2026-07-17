//! `WorkspaceFixture` — a tempdir + workspace path bundle. Drop the
//! fixture to clean the directory.
//!
//! Tests that need pre-seeded files use `with_files(...)`; tests that
//! seed their own files use `empty()`.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// Owns a `TempDir` and exposes the canonical workspace root path.
/// The `_tmp` field is private; the directory is removed when the
/// fixture drops.
pub struct WorkspaceFixture {
    tmp: TempDir,
    root: PathBuf,
}

impl WorkspaceFixture {
    /// Build an empty workspace root at `<tempdir>/ws/`.
    pub fn empty() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("ws");
        std::fs::create_dir_all(&root).expect("create ws");
        Self { tmp, root }
    }

    /// Build a workspace pre-seeded with the given relative-path files.
    /// Parent directories are created automatically.
    pub fn with_files<I, P, S>(files: I) -> Self
    where
        I: IntoIterator<Item = (P, S)>,
        P: AsRef<Path>,
        S: AsRef<[u8]>,
    {
        let fx = Self::empty();
        for (rel, bytes) in files {
            let abs = fx.root.join(rel.as_ref());
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent).expect("create parent");
            }
            std::fs::write(&abs, bytes.as_ref()).expect("seed file");
        }
        fx
    }

    /// Workspace root path. Pass to `LocalFilesystem::new(...)` /
    /// `LocalShellExecutor::new(...)`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Underlying tempdir path (parent of the workspace root). Tests
    /// that need an artifact root or scratch space outside the
    /// workspace use this.
    pub fn tempdir(&self) -> &Path {
        self.tmp.path()
    }

    /// Consume the fixture, returning the owned `TempDir` so the
    /// caller can extend its lifetime.
    pub fn into_tempdir(self) -> TempDir {
        self.tmp
    }
}
