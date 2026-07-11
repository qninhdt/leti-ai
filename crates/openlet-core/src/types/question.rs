//! `QuestionId` — strongly-typed identifier for an in-flight `ask_user`
//! question. Lives in `types/` (IO-free domain data) so `types/event.rs`
//! can reference it without depending on `runtime/`. The runtime
//! `question_registry` re-exports it for back-compat.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Strongly-typed question identifier (UUIDv7 — sortable by issue time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QuestionId(pub Uuid);

impl QuestionId {
    /// Mint a fresh UUIDv7-based id. Time-ordered so registry entries
    /// inserted close together stay clustered, which keeps DashMap
    /// shard locality reasonable under load.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for QuestionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for QuestionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for QuestionId {
    fn from(v: Uuid) -> Self {
        Self(v)
    }
}
