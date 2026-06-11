use std::path::PathBuf;

use async_trait::async_trait;

use crate::error::MemoryError;
use crate::types::agent::AgentId;
use crate::types::message::{Message, MessageId};
use crate::types::pagination::{Page, PageResult};
use crate::types::part::{Part, PartId};
use crate::types::permission::PermissionMode;
use crate::types::session::{SessionFilter, SessionId, SessionMeta, SessionStatus};

/// Persists sessions, messages, parts, and read history.
#[async_trait]
pub trait MemoryStore: Send + Sync + 'static {
    async fn create_session(
        &self,
        agent_id: AgentId,
        parent: Option<SessionId>,
    ) -> Result<SessionId, MemoryError>;

    /// Persist a fully-formed [`SessionMeta`] verbatim, preserving the
    /// caller-supplied `id`, `depth`, `permission_mode`, and parent link.
    ///
    /// Subagent spawning needs this: `plan_subagent_spawn` builds a child
    /// `SessionMeta` with the correct `depth` (for the depth-limit guard)
    /// and a pre-allocated id that later messages/parts are keyed on. The
    /// plain `create_session` mints a *fresh* id and hardcodes `depth = 0`,
    /// which would (a) orphan the seeded child messages under FK enforcement
    /// and (b) defeat depth enforcement on grandchildren.
    ///
    /// Default impl delegates to [`Self::create_session`] for stores that
    /// don't model `depth`/verbatim ids (e.g. test doubles); production
    /// stores override to insert the row as-is and return `meta.id`.
    async fn create_session_with_meta(&self, meta: SessionMeta) -> Result<SessionId, MemoryError> {
        self.create_session(meta.agent_id, meta.parent_session_id)
            .await
    }

    async fn get_session(&self, session: SessionId) -> Result<Option<SessionMeta>, MemoryError>;

    async fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, MemoryError>;

    /// Paginated `list_sessions`. Cloud stores serving many tenants must
    /// bound the result set; the default slices the unbounded
    /// [`Self::list_sessions`] by the opaque offset cursor so test
    /// doubles need no change. Production stores override with native
    /// `LIMIT/OFFSET` (or keyset) SQL.
    async fn list_sessions_paged(
        &self,
        filter: SessionFilter,
        page: Page,
    ) -> Result<PageResult<SessionMeta>, MemoryError> {
        let all = self.list_sessions(filter).await?;
        Ok(PageResult::from_slice(all, &page))
    }

    async fn update_status(
        &self,
        session: SessionId,
        status: SessionStatus,
        reason: &str,
    ) -> Result<(), MemoryError>;

    /// Updates the per-session `permission_mode`. Returns
    /// `SessionNotFound` if the session does not exist or is soft-deleted.
    async fn update_permission_mode(
        &self,
        session: SessionId,
        mode: PermissionMode,
    ) -> Result<(), MemoryError>;

    /// Switches the session's active agent slug, archiving the prior
    /// slug into `previous_agent_slug` so `ExitPlanMode` can restore it.
    /// `agent_slug` is stored as plain text (string) — `AgentSlug`
    /// validation lives in the typed registry, not the storage layer.
    /// Returns `SessionNotFound` if the session is missing or
    /// soft-deleted.
    async fn switch_agent(&self, session: SessionId, agent_slug: &str) -> Result<(), MemoryError>;

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

    /// Paginated `list_messages`. Same contract as
    /// [`Self::list_sessions_paged`]: default slices the unbounded list;
    /// production stores override with native paging.
    async fn list_messages_paged(
        &self,
        session: SessionId,
        page: Page,
    ) -> Result<PageResult<Message>, MemoryError> {
        let all = self.list_messages(session).await?;
        Ok(PageResult::from_slice(all, &page))
    }

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
    /// Persisted to `session_reads`.
    async fn record_read(&self, session: SessionId, path: PathBuf) -> Result<(), MemoryError>;
}
