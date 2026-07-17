//! Local-filesystem adapters.
//!
//! - `LocalFsArtifactStore`: per-session blob bucket with SQLite-backed
//!   metadata (sha256 key → file).
//! - `LocalFilesystem`: workspace-scoped `Filesystem` impl backing the
//!   file tools (`read`/`write`/`edit`/`list`/`glob`/`grep`) for laptop
//!   or single-tenant deployments.
//! - `SessionLogger`: per-session JSONL append log.

pub mod artifact_store;
pub mod filesystem;
pub mod redactor;
pub mod session_log;

pub use artifact_store::LocalFsArtifactStore;
pub use filesystem::LocalFilesystem;
pub use redactor::SecretRedactor;
pub use session_log::SessionLogger;
