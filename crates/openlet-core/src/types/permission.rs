use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::session::SessionId;

/// Permission mode (per-session, mutable via `POST /v1/session/:id/mode`).
///
/// Coarse enum — the full ruleset lives in `permissions.toml`. Plan
/// amendment §A makes this a session-level column.
///
/// Ordering: `ReadOnly < WorkspaceWrite < Danger`. `mode.permits(required)`
/// returns `true` iff the active mode is at least as permissive as `required`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    Serialize,
    Deserialize,
    ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    ReadOnly,
    #[default]
    WorkspaceWrite,
    Danger,
}

impl PermissionMode {
    /// `true` iff the active mode is at least as permissive as `required`.
    #[must_use]
    pub fn permits(self, required: Self) -> bool {
        self >= required
    }
}

/// One ruleset row — a permission pattern + the action to take.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub permission: String, // e.g. "bash:rm -rf*", "file:write:/tmp/*"
    pub action: PermissionAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionAction {
    Allow,
    Deny,
    Ask,
}

/// Identifier for a pending ask (UUIDv4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AskId(pub Uuid);

impl AskId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for AskId {
    fn default() -> Self {
        Self::new()
    }
}

/// A request the runtime is asking the user (or a plugin) to decide on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRequest {
    pub permission: String,
    pub reason: Option<String>,
    pub timeout: Option<Duration>,
}

/// Outcome of a permission check.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny {
        feedback: Option<String>,
    },
    /// Pending user input — caller awaits via `PermissionManager::reply`.
    Pending {
        ask_id: AskId,
    },
}

impl Decision {
    /// Stable wire label. Used by the protocol DTO so the wire format
    /// only carries the outcome — never the (optionally-PII) feedback
    /// or ask-id payload.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny { .. } => "deny",
            Self::Pending { .. } => "pending",
        }
    }
}

/// Per-call context attached to every `PermissionManager::check` call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionCtx {
    pub session_id: SessionId,
    pub mode: PermissionMode,
}

/// Scope at which an "always" rule is recorded (§A new method).
///
/// `Global` rules apply to every session in the host. The other variants
/// narrow the rule to a single workspace, agent, or session — the manager
/// keeps the discriminant on the compiled rule and only consults rules
/// whose scope matches the active [`PermissionCtx`] at evaluation time.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum AlwaysScope {
    Global,
    Workspace { path: PathBuf },
    Agent { id: String },
    Session { id: SessionId },
}
