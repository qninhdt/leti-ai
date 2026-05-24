use std::path::PathBuf;

use async_trait::async_trait;

use crate::error::MemoryError;
use crate::types::agent::AgentId;
use crate::types::message::{Message, MessageId};
use crate::types::part::{Part, PartId};
use crate::types::permission::PermissionMode;
use crate::types::session::{SessionFilter, SessionId, SessionMeta, SessionStatus};

/// Persists sessions, messages, parts, and read history.
///
/// Phase 2 implements `SqliteMemoryStore`. Trait surface includes §A
/// additions: `list_sessions`, `delete_session` (soft), `upsert_part` (for
/// streaming text appends), `record_read` (read-history table).
#[async_trait]
pub trait MemoryStore: Send + Sync + 'static {
    async fn create_session(
        &self,
        agent_id: AgentId,
        parent: Option<SessionId>,
    ) -> Result<SessionId, MemoryError>;

    async fn get_session(&self, session: SessionId) -> Result<Option<SessionMeta>, MemoryError>;

    async fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, MemoryError>;

    async fn update_status(
        &self,
        session: SessionId,
        status: SessionStatus,
        reason: &str,
    ) -> Result<(), MemoryError>;

    /// Updates the per-session `permission_mode` (§A F27). Returns
    /// `SessionNotFound` if the session does not exist or is soft-deleted.
    async fn update_permission_mode(
        &self,
        session: SessionId,
        mode: PermissionMode,
    ) -> Result<(), MemoryError>;

    /// Replaces the integrator-owned `extensions` JSON blob on a session.
    /// Core stays auth-blind — schema lives entirely in the integrator
    /// (e.g. `{"user_id": "u_123"}`). Returns `SessionNotFound` if the
    /// session is missing or soft-deleted.
    async fn update_session_extensions(
        &self,
        session: SessionId,
        extensions: serde_json::Value,
    ) -> Result<(), MemoryError>;

    /// Soft-delete: sets status=cancelled + deleted_at.
    async fn delete_session(&self, session: SessionId) -> Result<(), MemoryError>;

    async fn append_message(
        &self,
        session: SessionId,
        msg: Message,
    ) -> Result<MessageId, MemoryError>;

    async fn append_part(&self, msg: MessageId, part: Part) -> Result<PartId, MemoryError>;

    /// Replace an existing part (used by streaming text deltas appending
    /// to an in-progress Text part).
    async fn upsert_part(
        &self,
        msg: MessageId,
        part_id: PartId,
        part: Part,
    ) -> Result<(), MemoryError>;

    async fn list_messages(&self, session: SessionId) -> Result<Vec<Message>, MemoryError>;

    /// Lists every persisted part for `msg`, in append order. Used by
    /// the multi-step turn loop to harvest tool_calls from the latest
    /// assistant message and by the projection layer to rebuild
    /// LLM-shape messages between turns.
    async fn list_parts(
        &self,
        session: SessionId,
        msg: MessageId,
    ) -> Result<Vec<Part>, MemoryError>;

    /// Records that the agent read a path during this session.
    /// Persisted to `session_reads` (Phase 2 schema, §F).
    async fn record_read(&self, session: SessionId, path: PathBuf) -> Result<(), MemoryError>;
}
