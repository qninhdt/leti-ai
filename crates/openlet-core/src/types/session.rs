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
}

/// Filter for `MemoryStore::list_sessions` (added in §A).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionFilter {
    pub status: Option<SessionStatus>,
    pub agent_id: Option<AgentId>,
    pub include_deleted: bool,
}
