//! SQLite `MemoryStore` impl.
//!
//! Phase 1 stub. Phase 2 fills in the schema, migrations, and CRUD.

use std::path::PathBuf;

use async_trait::async_trait;
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::error::MemoryError;
use openlet_core::types::message::{Message, MessageId};
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::session::{SessionFilter, SessionId, SessionMeta, SessionStatus};

#[derive(Debug, Default)]
pub struct SqliteMemoryStore;

impl SqliteMemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl MemoryStore for SqliteMemoryStore {
    async fn create_session(
        &self,
        _agent_id: &str,
        _parent: Option<SessionId>,
    ) -> Result<SessionId, MemoryError> {
        Err(MemoryError::Unimplemented)
    }

    async fn get_session(
        &self,
        _session: SessionId,
    ) -> Result<Option<SessionMeta>, MemoryError> {
        Err(MemoryError::Unimplemented)
    }

    async fn list_sessions(
        &self,
        _filter: SessionFilter,
    ) -> Result<Vec<SessionMeta>, MemoryError> {
        Err(MemoryError::Unimplemented)
    }

    async fn update_status(
        &self,
        _session: SessionId,
        _status: SessionStatus,
        _reason: &str,
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Unimplemented)
    }

    async fn delete_session(&self, _session: SessionId) -> Result<(), MemoryError> {
        Err(MemoryError::Unimplemented)
    }

    async fn append_message(
        &self,
        _session: SessionId,
        _msg: Message,
    ) -> Result<MessageId, MemoryError> {
        Err(MemoryError::Unimplemented)
    }

    async fn append_part(
        &self,
        _msg: MessageId,
        _part: Part,
    ) -> Result<PartId, MemoryError> {
        Err(MemoryError::Unimplemented)
    }

    async fn upsert_part(
        &self,
        _msg: MessageId,
        _part_id: PartId,
        _part: Part,
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Unimplemented)
    }

    async fn list_messages(
        &self,
        _session: SessionId,
    ) -> Result<Vec<Message>, MemoryError> {
        Err(MemoryError::Unimplemented)
    }

    async fn record_read(
        &self,
        _session: SessionId,
        _path: PathBuf,
    ) -> Result<(), MemoryError> {
        Err(MemoryError::Unimplemented)
    }
}
