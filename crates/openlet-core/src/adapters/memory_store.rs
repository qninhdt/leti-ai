use std::path::PathBuf;

use async_trait::async_trait;

use crate::error::MemoryError;
use crate::types::message::{Message, MessageId};
use crate::types::part::{Part, PartId};
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
        agent_id: &str,
        parent: Option<SessionId>,
    ) -> Result<SessionId, MemoryError>;

    async fn get_session(&self, session: SessionId)
        -> Result<Option<SessionMeta>, MemoryError>;

    async fn list_sessions(
        &self,
        filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, MemoryError>;

    async fn update_status(
        &self,
        session: SessionId,
        status: SessionStatus,
        reason: &str,
    ) -> Result<(), MemoryError>;

    /// Soft-delete: sets status=cancelled + deleted_at.
    async fn delete_session(&self, session: SessionId) -> Result<(), MemoryError>;

    async fn append_message(
        &self,
        session: SessionId,
        msg: Message,
    ) -> Result<MessageId, MemoryError>;

    async fn append_part(
        &self,
        msg: MessageId,
        part: Part,
    ) -> Result<PartId, MemoryError>;

    /// Replace an existing part (used by streaming text deltas appending
    /// to an in-progress Text part).
    async fn upsert_part(
        &self,
        msg: MessageId,
        part_id: PartId,
        part: Part,
    ) -> Result<(), MemoryError>;

    async fn list_messages(&self, session: SessionId)
        -> Result<Vec<Message>, MemoryError>;

    /// Records that the agent read a path during this session.
    /// Persisted to `session_reads` (Phase 2 schema, §F).
    async fn record_read(
        &self,
        session: SessionId,
        path: PathBuf,
    ) -> Result<(), MemoryError>;
}
