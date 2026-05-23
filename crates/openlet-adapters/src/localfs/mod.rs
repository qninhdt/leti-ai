//! Local-filesystem `ArtifactStore` impl.
//!
//! Layout: `<root>/<session_id>/<sha256(key).hex>`. Metadata persisted in
//! the `artifacts` SQLite table so listing is O(1) without directory scan.
//! Keys are sanitized to refuse `..` and absolute paths so a malicious key
//! cannot escape the per-session directory.

pub mod artifact_store;
pub mod session_log;

pub use artifact_store::LocalFsArtifactStore;
pub use session_log::SessionLogger;
