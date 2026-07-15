use std::path::PathBuf;

use async_trait::async_trait;

use crate::error::MemoryError;
use crate::types::agent::AgentId;
use crate::types::message::{Message, MessageId};
use crate::types::pagination::{Page, PageResult};
use crate::types::part::{Part, PartId, ReminderKind};
use crate::types::permission::PermissionMode;
use crate::types::session::{SessionFilter, SessionId, SessionMeta, SessionStatus};

/// Scope of a read observation. A `Full` read fingerprints the entire file;
/// a `Range` read only observed part of it and must never claim the unseen
/// remainder is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadScope {
    Full,
    Range,
}

impl ReadScope {
    /// Stable wire label persisted in the `session_reads.scope` column.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Range => "range",
        }
    }

    /// Parse the persisted label; unknown values fail safe to `Range` so a
    /// forward-compat row never claims unseen content was fully observed.
    #[must_use]
    pub fn from_label(s: &str) -> Self {
        match s {
            "range" => Self::Range,
            "full" => Self::Full,
            _ => Self::Range,
        }
    }
}

/// Content fingerprint over file bytes. A stable, dependency-free FNV-1a hash
/// — change detection only needs difference, not cryptographic strength. Both
/// the read tool (recording an observation) and the workspace-delta reminder
/// producer (checking the current on-disk content) MUST use this one function
/// so a recorded fingerprint compares equal to a later check of identical bytes.
#[must_use]
pub fn fingerprint_bytes(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a:{hash:016x}")
}

/// A durable record that the agent observed a file during a session. Unlike
/// the legacy path-only read row, this carries a content `fingerprint` so the
/// workspace-delta reminder producer can detect that a previously-observed
/// file changed or was deleted. `fingerprint: None` means "seen, content
/// unknown" (legacy row or bare path record) and never reports a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadObservation {
    pub session_id: SessionId,
    pub path: PathBuf,
    pub fingerprint: Option<String>,
    pub scope: ReadScope,
}

/// Durable parent-notification payload for a background subagent terminal
/// state. The output is kept in the typed reminder part and never emitted in
/// lifecycle SSE frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundTaskSettlement {
    pub parent_session_id: SessionId,
    pub task_id: String,
    pub child_session_id: SessionId,
    pub status: String,
    pub output: String,
    pub cost_usd: Option<String>,
}

/// A settlement claimed from the durable delivery outbox. The lease token is
/// opaque and must be presented when acknowledging the parent turn so a stale
/// worker cannot acknowledge a delivery reclaimed after a crash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedBackgroundTaskSettlement {
    pub settlement: BackgroundTaskSettlement,
    pub lease_id: String,
}

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

    /// Atomically append a reminder-only message and reserve each reminder's
    /// durable delivery identity. Production stores override this so racing
    /// request preparations cannot duplicate `(kind, stable_key, epoch)` and
    /// a part failure cannot leave an empty user-role message behind.
    ///
    /// The default keeps non-production stores source-compatible. It is not
    /// race-proof, but still preserves the all-or-error API contract expected
    /// by deterministic test stores.
    async fn append_runtime_reminders(
        &self,
        session: SessionId,
        msg: Message,
        parts: Vec<Part>,
    ) -> Result<Option<(MessageId, Vec<PartId>)>, MemoryError> {
        if parts.is_empty() {
            return Ok(None);
        }
        let mid = self.append_message(session, msg).await?;
        let mut ids = Vec::with_capacity(parts.len());
        for part in parts {
            ids.push(self.append_part(mid, part).await?);
        }
        Ok(Some((mid, ids)))
    }

    /// Atomically reserve the exactly-once typed notification for a
    /// background task settlement. Stores with a delivery table override the
    /// generic append to make retries idempotent across process restarts.
    async fn append_background_task_settled(
        &self,
        settlement: BackgroundTaskSettlement,
    ) -> Result<Option<(MessageId, Vec<PartId>)>, MemoryError> {
        let message = Message {
            id: MessageId::new(),
            session_id: settlement.parent_session_id,
            role: crate::types::message::Role::User,
            created_at: chrono::Utc::now(),
        };
        let body = format!(
            "Background subagent task {} ({}) settled with status {}.\n{}{}",
            settlement.task_id,
            settlement.child_session_id,
            settlement.status,
            settlement.output,
            settlement
                .cost_usd
                .as_deref()
                .map_or(String::new(), |cost| format!("\nCost: ${cost}"))
        );
        self.append_runtime_reminders(
            settlement.parent_session_id,
            message,
            vec![Part::RuntimeReminder {
                id: PartId::new(),
                reminder_kind: ReminderKind::BackgroundTaskSettled,
                stable_key: format!("task:{}", settlement.task_id),
                content: body,
                projection_epoch: 0,
            }],
        )
        .await
    }

    /// Atomically lease background settlements whose durable reminder exists
    /// but whose parent turn has not reached a terminal state. Expired leases
    /// are reclaimable after a worker/process crash.
    async fn claim_background_task_settlements(
        &self,
        _parent_session_id: Option<SessionId>,
        _task_id: Option<&str>,
    ) -> Result<Vec<ClaimedBackgroundTaskSettlement>, MemoryError> {
        Ok(Vec::new())
    }

    /// Acknowledge a delivery only after its leased parent turn exits. The
    /// lease token prevents a stale worker from completing a newer attempt.
    async fn acknowledge_background_task_settlement(
        &self,
        _parent_session_id: SessionId,
        _task_id: &str,
        _lease_id: &str,
    ) -> Result<(), MemoryError> {
        Ok(())
    }

    /// Return a failed parent-turn attempt to the durable queue. Only the
    /// current lease holder may release it, preventing stale workers from
    /// reintroducing a delivery already handled by a later attempt.
    async fn release_background_task_settlement(
        &self,
        _parent_session_id: SessionId,
        _task_id: &str,
        _lease_id: &str,
    ) -> Result<(), MemoryError> {
        Ok(())
    }

    /// Renew a single live parent-turn lease. The caller must supply the
    /// current token, so another process cannot extend a reclaimed delivery.
    async fn renew_background_task_settlement_lease(
        &self,
        _parent_session_id: SessionId,
        _task_id: &str,
        _lease_id: &str,
    ) -> Result<(), MemoryError> {
        Ok(())
    }

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

    /// Records a content-fingerprinted read observation, atomically upserting
    /// by `(session_id, path)`. This is the durable contract the
    /// workspace-delta reminder producer reads to detect changed/deleted
    /// files across restarts.
    ///
    /// Default impl degrades to the legacy path-only [`Self::record_read`] so
    /// test doubles and stores that don't model fingerprints keep compiling;
    /// production stores override to persist the fingerprint + scope.
    async fn record_observation(&self, obs: ReadObservation) -> Result<(), MemoryError> {
        self.record_read(obs.session_id, obs.path).await
    }

    /// Lists every durable read observation for a session, used on restart to
    /// re-hydrate changed-file detection. Default impl returns empty so stores
    /// that don't model observations simply report no prior state.
    async fn list_observations(
        &self,
        _session: SessionId,
    ) -> Result<Vec<ReadObservation>, MemoryError> {
        Ok(Vec::new())
    }
}
