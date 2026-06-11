//! Workspace resolver — maps incoming workspace ids to per-tenant
//! [`AppState`] instances.
//!
//! Cloud deployments serve N workspaces from a single binary; each
//! workspace gets its own SQLite path, BYOK provider stack, and
//! plugin set. This trait is the seam: the local binary uses
//! [`StaticWorkspaceResolver`] (one workspace, hard-coded), the cloud
//! binary plugs its own resolver that reads from a control plane.
//!
//! ## Caching contract
//!
//! Implementations MAY cache resolved [`AppState`] instances. The
//! invalidation protocol is: when an integrator's control plane mutates
//! a workspace's BYOK keys / plugin set / quota, it MUST evict the
//! cached entry so the next request sees the new state. Eviction is
//! integrator-side (we don't ship a control-plane hook) — typical
//! impls expose an `invalidate(workspace_id)` method or use a
//! short-TTL cache.
//!
//! ## Per-workspace isolation contract
//!
//! Per-workspace SQLite databases MUST live in distinct subdirectories
//! so a path-traversal bug in workspace id parsing cannot leak data
//! across tenants. Use [`workspace_data_root`] to compute the per-
//! workspace path; the helper validates the id and rejects path
//! separators / `..` components.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use crate::app_state::AppState;
use crate::auth::AuthPrincipal;

/// Errors surfaced by [`WorkspaceResolver::resolve`]. Mapped to HTTP
/// status by the routing middleware.
#[derive(Debug, thiserror::Error)]
pub enum WorkspaceError {
    /// Caller's workspace id syntactically malformed (path traversal,
    /// empty string, illegal characters). 400.
    #[error("invalid workspace id: {0}")]
    Invalid(String),
    /// Workspace id is well-formed but unknown / soft-deleted. 404.
    #[error("workspace not found: {0}")]
    NotFound(String),
    /// Resolver-internal failure (control-plane unreachable, DB
    /// lookup error). 503.
    #[error("workspace lookup failed: {0}")]
    LookupFailed(String),
    /// Caller authenticated but does not own / is not authorized for the
    /// target workspace. 403.
    #[error("forbidden: {0}")]
    Forbidden(String),
}

/// Resolver interface. The default impl ([`StaticWorkspaceResolver`])
/// always returns the same state; cloud impls read from a control plane.
///
/// The authenticated [`AuthPrincipal`] is passed so the resolver can
/// enforce ownership — a caller may only resolve a workspace they own or
/// are authorized for. The local single-tenant impl ignores it; cloud
/// impls return [`WorkspaceError::Forbidden`] on a mismatch.
///
/// Cloud-readiness (adapter-contract audit): finalized — the principal +
/// opaque `workspace_id` + `Forbidden`/`NotFound`/`Invalid`/`LookupFailed`
/// error set are sufficient for a control-plane-backed resolver that maps
/// caller → agent-workspaces. No `std::path` leaks into the trait;
/// per-workspace data roots are computed impl-side via
/// [`workspace_data_root`]. No signature change needed beyond the
/// principal threading added in the auth phase.
#[async_trait]
pub trait WorkspaceResolver: Send + Sync + 'static {
    async fn resolve(
        &self,
        principal: &AuthPrincipal,
        workspace_id: &str,
    ) -> Result<Arc<AppState>, WorkspaceError>;
}

/// Single-tenant resolver — local binary uses this so the existing
/// behavior (one workspace, no header required) keeps working.
#[derive(Clone)]
pub struct StaticWorkspaceResolver {
    state: Arc<AppState>,
}

impl StaticWorkspaceResolver {
    #[must_use]
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl WorkspaceResolver for StaticWorkspaceResolver {
    async fn resolve(
        &self,
        _principal: &AuthPrincipal,
        _workspace_id: &str,
    ) -> Result<Arc<AppState>, WorkspaceError> {
        // Single-tenant: one owner, any well-formed id resolves to the
        // same state. Ownership is trivially satisfied locally. Cloud
        // deployments override this with a real lookup + ownership check.
        Ok(self.state.clone())
    }
}

/// Per-workspace data root — `{base}/workspaces/{ws_id}/`. Validates
/// `ws_id` rejects path separators and `..` so a malicious caller
/// cannot land outside the workspaces directory. Isolation gate.
///
/// Returns `WorkspaceError::Invalid` for ids that contain `/`, `\`,
/// `..`, NUL, control characters, or are empty.
pub fn workspace_data_root(base: &Path, ws_id: &str) -> Result<PathBuf, WorkspaceError> {
    if ws_id.is_empty() {
        return Err(WorkspaceError::Invalid("empty workspace id".into()));
    }
    // Reject path separators + traversal markers + control chars.
    if ws_id.contains('/')
        || ws_id.contains('\\')
        || ws_id.contains('\0')
        || ws_id == "."
        || ws_id == ".."
        || ws_id.chars().any(|c| c.is_control())
    {
        return Err(WorkspaceError::Invalid(format!(
            "workspace id contains illegal characters: {ws_id:?}"
        )));
    }
    // Cap length so a pathological id can't blow past filesystem
    // limits (typical FS cap is 255 bytes for a single component).
    if ws_id.len() > 128 {
        return Err(WorkspaceError::Invalid(format!(
            "workspace id too long: {} bytes",
            ws_id.len()
        )));
    }
    Ok(base.join("workspaces").join(ws_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_data_root_rejects_traversal() {
        let base = PathBuf::from("/tmp");
        assert!(workspace_data_root(&base, "..").is_err());
        assert!(workspace_data_root(&base, ".").is_err());
        assert!(workspace_data_root(&base, "../sneaky").is_err());
        assert!(workspace_data_root(&base, "ws/1").is_err());
        assert!(workspace_data_root(&base, "ws\\1").is_err());
        assert!(workspace_data_root(&base, "").is_err());
        assert!(workspace_data_root(&base, "ws\0").is_err());
    }

    #[test]
    fn workspace_data_root_accepts_uuid() {
        let base = PathBuf::from("/tmp");
        let p = workspace_data_root(&base, "01923-abcd-ef").unwrap();
        assert!(p.ends_with("workspaces/01923-abcd-ef"));
    }

    #[test]
    fn workspace_data_root_caps_length() {
        let base = PathBuf::from("/tmp");
        let too_long = "a".repeat(200);
        assert!(workspace_data_root(&base, &too_long).is_err());
    }
}
