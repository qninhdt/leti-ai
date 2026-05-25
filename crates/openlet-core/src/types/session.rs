use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::agent::AgentId;
use super::permission::PermissionMode;

/// Strongly-typed session identifier (UUIDv4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

impl SessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for SessionId {
    fn from(v: Uuid) -> Self {
        Self(v)
    }
}

/// Lifecycle status surfaced via `session.status` events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Idle,
    Running,
    Cancelling,
    Cancelled,
    Errored,
}

/// Session-level metadata persisted in the memory store.
///
/// `extensions` is an opaque JSON blob the integrator owns. Core stays
/// auth-blind — `extensions["user_id"]` (or any other shape) is the
/// integrator's responsibility, not core's. Default = `Value::Null`.
///
/// `capabilities` declares which interactive frontend affordances the
/// session's caller supports. Headless-cloud callers leave the default
/// (every flag `false`), so tools that require an interactive frontend
/// (e.g. `ask_user`) return a synchronous error instead of blocking
/// indefinitely on a UI that will never answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,
    pub agent_id: AgentId,
    pub status: SessionStatus,
    pub permission_mode: PermissionMode,
    pub parent_session_id: Option<SessionId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub version: String,
    #[serde(default)]
    pub extensions: serde_json::Value,
    #[serde(default)]
    pub capabilities: SessionCapabilities,
    /// Slug of the agent PROFILE the session is currently running.
    /// `None` ⇒ runtime falls back to the default profile (`general`).
    /// Distinct from `agent_id` (UUID principal that owns the
    /// workspace); plan mode swaps THIS field, not the principal.
    #[serde(default)]
    pub current_agent_slug: Option<String>,
    /// Slug of the agent profile the session was on BEFORE the current
    /// `current_agent_slug`. Set by `MemoryStore::switch_agent` on
    /// every transition so `ExitPlanMode` can restore the prior profile
    /// without an explicit session-level "plan_mode" flag (plan mode IS
    /// the agent profile). `None` when the session has never switched
    /// profiles.
    #[serde(default)]
    pub previous_agent_slug: Option<String>,
}

/// Frontend affordances the session's caller exposes. Default = every
/// flag `false` so headless-cloud sessions are safe by construction —
/// interactive tools (`ask_user`) return a synchronous error rather
/// than blocking on a UI that will never reply.
///
/// TUI / integrator binaries that drive a real human flip the relevant
/// flag to `true` at session create.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SessionCapabilities {
    /// Caller can answer interactive `ask_user` prompts via the
    /// `POST /v1/sessions/:id/question/answer` endpoint.
    #[serde(default)]
    pub user_questions: bool,
}

/// Filter for `MemoryStore::list_sessions` (added in §A).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionFilter {
    pub status: Option<SessionStatus>,
    pub agent_id: Option<AgentId>,
    pub include_deleted: bool,
}
