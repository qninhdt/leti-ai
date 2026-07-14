//! Minimal in-memory `MemoryStore` for tests that don't need SQLite
//! persistence. Tracks messages and parts per session; assigns
//! monotonic `seq` values within a session so projection tests can
//! assert ordering. Race tests that hammer monotonic-seq use the real
//! `SqliteMemoryStore` instead.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::error::MemoryError;
use openlet_core::types::agent::AgentId;
use openlet_core::types::message::{Message, MessageId};
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::permission::PermissionMode;
use openlet_core::types::session::{SessionFilter, SessionId, SessionMeta, SessionStatus};

#[derive(Default)]
pub struct MockMemoryStore {
    sessions: Mutex<HashMap<SessionId, SessionMeta>>,
    messages: Mutex<HashMap<SessionId, Vec<Message>>>,
    parts: Mutex<HashMap<MessageId, Vec<Part>>>,
}

impl MockMemoryStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn message_count(&self, session: SessionId) -> usize {
        self.messages
            .lock()
            .unwrap()
            .get(&session)
            .map_or(0, Vec::len)
    }

    /// Insert a `SessionMeta` directly so tests that exercise
    /// parent/child resolution (e.g. `send_message` hierarchy scoping) can
    /// pre-seed a session tree without driving `create_session`.
    pub fn put_session(&self, meta: SessionMeta) {
        self.sessions.lock().unwrap().insert(meta.id, meta);
    }
}

#[async_trait]
impl MemoryStore for MockMemoryStore {
    async fn create_session(
        &self,
        _agent_id: AgentId,
        _parent: Option<SessionId>,
    ) -> Result<SessionId, MemoryError> {
        let id = SessionId::new();
        // Don't try to construct a real SessionMeta — the few tests that
        // care use `MemoryStore::get_session = None` and rely on side
        // effects instead. Keep the entry empty.
        self.messages.lock().unwrap().insert(id, Vec::new());
        Ok(id)
    }

    async fn get_session(&self, session: SessionId) -> Result<Option<SessionMeta>, MemoryError> {
        Ok(self.sessions.lock().unwrap().get(&session).cloned())
    }

    async fn list_sessions(&self, _filter: SessionFilter) -> Result<Vec<SessionMeta>, MemoryError> {
        Ok(self.sessions.lock().unwrap().values().cloned().collect())
    }

    async fn update_status(
        &self,
        _session: SessionId,
        _status: SessionStatus,
        _reason: &str,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn update_permission_mode(
        &self,
        _session: SessionId,
        _mode: PermissionMode,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn switch_agent(
        &self,
        _session: SessionId,
        _agent_slug: &str,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn update_session_extensions(
        &self,
        _session: SessionId,
        _extensions: serde_json::Value,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn delete_session(&self, _session: SessionId) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn append_message(
        &self,
        session: SessionId,
        msg: Message,
    ) -> Result<MessageId, MemoryError> {
        let id = msg.id;
        self.messages
            .lock()
            .unwrap()
            .entry(session)
            .or_default()
            .push(msg);
        Ok(id)
    }
    async fn append_part(&self, msg: MessageId, part: Part) -> Result<PartId, MemoryError> {
        let pid = part.id();
        self.parts
            .lock()
            .unwrap()
            .entry(msg)
            .or_default()
            .push(part);
        Ok(pid)
    }
    async fn upsert_part(
        &self,
        msg: MessageId,
        pid: PartId,
        part: Part,
    ) -> Result<(), MemoryError> {
        let mut g = self.parts.lock().unwrap();
        let v = g.entry(msg).or_default();
        if let Some(slot) = v.iter_mut().find(|p| p.id() == pid) {
            *slot = part;
        } else {
            v.push(part);
        }
        Ok(())
    }
    async fn list_messages(&self, session: SessionId) -> Result<Vec<Message>, MemoryError> {
        Ok(self
            .messages
            .lock()
            .unwrap()
            .get(&session)
            .cloned()
            .unwrap_or_default())
    }
    async fn list_parts(
        &self,
        _session: SessionId,
        msg: MessageId,
    ) -> Result<Vec<Part>, MemoryError> {
        Ok(self
            .parts
            .lock()
            .unwrap()
            .get(&msg)
            .cloned()
            .unwrap_or_default())
    }
    async fn record_read(&self, _session: SessionId, _path: PathBuf) -> Result<(), MemoryError> {
        Ok(())
    }
}
