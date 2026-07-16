//! `Filesystem` adapter — workspace-scoped file operations behind a
//! swap seam.
//!
//! Six built-in tools (`read`, `write`, `edit`, `list`, `glob`, `grep`)
//! used to call `tokio::fs` directly. That made openlet-ai-core
//! laptop-only by accident: there was no place to plug in cloud storage
//! (S3 via openlet `file-service`), a sandboxed FUSE mount, or a
//! mocking layer for tests.
//!
//! `Filesystem` is the missing seam. Tools call `ctx.fs.*`; the impl
//! decides what "the workspace" actually is. The local impl
//! (`openlet_adapters::localfs::LocalFilesystem`) keeps today's
//! tokio-fs behavior with the deepest-existing-ancestor canonicalize
//! trick. A future cloud impl maps the same calls onto
//! file-service gRPC + presigned URLs.
//!
//! Invariants the trait imposes on every impl:
//! - All paths are workspace-relative. Absolute paths and `..` escapes
//!   are rejected with `FsError::OutsideWorkspace`.
//! - `read` / `stat` are read-only and parallel-safe.
//! - `write` overwrites atomically when possible. Tools that need
//!   create-only / append semantics layer their own checks on top.
//! - `glob` / `grep` are best-effort: results are bounded by
//!   `max_results`, but the order is impl-defined unless the caller
//!   passes a `GlobSort`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::error::FsError;

/// Object-safe trait the runtime injects via `ToolCtx::fs`.
#[async_trait]
pub trait Filesystem: Send + Sync + 'static {
    /// Stable lock namespace for a workspace-relative path. Adapters with a
    /// real workspace identity should override this; the default is safe for
    /// custom adapters because it includes the concrete adapter type.
    fn scheduling_key(&self, path: &Path) -> String {
        format!(
            "{}:{}",
            std::any::type_name::<Self>(),
            crate::tools::scheduler::normalize_path(path).display()
        )
    }
    /// Read a file in full or a `(start, len)` byte range. Returns
    /// `FsError::Binary` if the impl deems the content non-text and
    /// the caller did not opt in via `ReadOpts::allow_binary` (handled
    /// at the tool layer, not here — adapters return raw bytes).
    async fn read(&self, path: &Path, range: Option<ByteRange>) -> Result<Bytes, FsError>;

    /// Stat a path. `FsError::NotFound` if absent.
    async fn stat(&self, path: &Path) -> Result<FileMeta, FsError>;

    /// Existence check. Never errors — returns `false` on any IO
    /// failure (callers that need the distinction use `stat`).
    async fn exists(&self, path: &Path) -> bool;

    /// Atomic-when-possible write. Creates parent dirs as needed.
    async fn write(&self, path: &Path, body: Bytes, opts: WriteOpts) -> Result<FileMeta, FsError>;

    /// Shallow directory listing — children of `path` only, no
    /// recursion. Sorted by name ascending.
    async fn list(&self, path: &Path) -> Result<Vec<DirEntry>, FsError>;

    /// Glob for paths matching `pattern`. The pattern is interpreted
    /// relative to the workspace root. Implementations honor
    /// `opts.respect_gitignore` on a best-effort basis (local impl
    /// does; cloud impl falls back to its index's view).
    async fn glob(&self, pattern: &str, opts: GlobOpts) -> Result<Vec<PathBuf>, FsError>;

    /// Recursive content search. Hits are bounded by
    /// `args.max_hits`; lines longer than `args.max_line_chars` are
    /// truncated.
    async fn grep(&self, args: GrepArgs) -> Result<Vec<GrepHit>, FsError>;

    /// Remove a file or empty directory. `FsError::NotFound` if the path
    /// does not exist. Implementations reject recursive directory removal
    /// here — callers that need `rm -r` walk + remove leaf-first through
    /// this method so the workspace-boundary check runs on every path.
    async fn remove(&self, path: &Path) -> Result<(), FsError>;

    /// Rename / move `from` to `to`. Both paths are workspace-relative and
    /// boundary-checked. Overwrites `to` when it exists (POSIX `rename`
    /// semantics). `FsError::NotFound` if `from` is absent.
    async fn rename(&self, from: &Path, to: &Path) -> Result<(), FsError>;
}

/// `(start, len)` byte slice. `start` is 0-indexed; `len = 0` means
/// "to end". Adapters validate against file size and clamp len.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: u64,
    pub len: u64,
}

/// Metadata returned by `stat` / `write`. `is_binary` is a heuristic
/// (NUL byte in first 8 KiB for the local impl); cloud impls should
/// reuse Magika's classification when available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMeta {
    pub size: u64,
    pub mtime_ms: i64,
    pub is_binary: bool,
    pub sha256: Option<String>,
}

/// One entry in a directory listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: Option<u64>,
}

/// Caller-provided options for `write`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteOpts {
    /// Refuse if the path already exists.
    pub create_new: bool,
    /// `true` (default) — write to a sibling tempfile and rename. Some
    /// adapters (e.g. presigned PUT to S3) cannot honor this and will
    /// degrade to a single-shot upload; they should set
    /// `FileMeta.atomic = false` on the returned meta. We don't expose
    /// that yet; the field is reserved.
    pub atomic: bool,
    /// Append `body` to the file instead of truncating. Backs shell
    /// `>>` and `tee -a`. When `true`, `atomic` is ignored (append is
    /// inherently a read-modify-write, not a whole-file swap) and
    /// `create_new` still refuses a pre-existing target. A missing file
    /// is created.
    pub append: bool,
}

impl Default for WriteOpts {
    fn default() -> Self {
        Self {
            create_new: false,
            atomic: true,
            append: false,
        }
    }
}

/// Caller-provided options for `glob`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlobOpts {
    pub respect_gitignore: bool,
    pub max_results: usize,
    pub sort: GlobSort,
}

impl Default for GlobOpts {
    fn default() -> Self {
        Self {
            respect_gitignore: true,
            max_results: 100,
            sort: GlobSort::MtimeDesc,
        }
    }
}

/// How `glob` orders results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GlobSort {
    PathAsc,
    MtimeDesc,
}

/// Caller-provided arguments for `grep`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrepArgs {
    pub pattern: String,
    pub path_glob: Option<String>,
    pub case_insensitive: bool,
    pub max_hits: usize,
    pub max_line_chars: usize,
}

impl Default for GrepArgs {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            path_glob: None,
            case_insensitive: false,
            max_hits: 250,
            max_line_chars: 2000,
        }
    }
}

/// One match returned by `grep`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrepHit {
    pub path: PathBuf,
    pub line: u64,
    pub text: String,
}
