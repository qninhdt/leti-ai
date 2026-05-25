//! In-process registry of running subagent tasks.
//!
//! Each `start` returns a [`TaskId`] backed by a [`TaskHandle`] holding
//! the cancellation token, status, output buffer, and cost. Quotas are
//! enforced per ROOT session (every nested descendant counts toward the
//! same root's bucket) so a depth-3 fan-out can't bypass the cap by
//! spreading work across grandchildren.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use dashmap::DashMap;
use rust_decimal::Decimal;
use thiserror::Error;
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::types::session::SessionId;

/// Per-task output cap. Mirrors the F4.10 contract — once a task's
/// concatenated output exceeds this, additional appends are discarded
/// and the buffer is replaced with a single `[truncated]` sentinel so
/// memory is bounded for adversarial subagent runs.
pub const MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

/// Default per-root-session quota for in-flight subagent tasks.
/// Overridable via `OPENLET_SUBAGENT_MAX_PER_SESSION`.
pub const DEFAULT_MAX_PER_SESSION: usize = 32;

/// Default maximum nesting depth. Top-level user sessions are depth 0;
/// `subagent_task` calls increment by 1. Overridable via
/// `OPENLET_SUBAGENT_MAX_DEPTH`.
pub const DEFAULT_MAX_DEPTH: u8 = 3;

/// Task identifier — UUIDv4 newtype. Stable across resume/poll calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub Uuid);

impl TaskId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskStatus {
    Running,
    Finished,
    Cancelled,
    Failed(String),
}

impl TaskStatus {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Finished | Self::Cancelled | Self::Failed(_))
    }

    /// Stable wire label. Used by `task_status` tool + SSE.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Finished => "finished",
            Self::Cancelled => "cancelled",
            Self::Failed(_) => "failed",
        }
    }
}

/// Owned handle to a running subagent task. Cloning is cheap (Arc).
#[derive(Clone)]
pub struct TaskHandle {
    pub status: Arc<RwLock<TaskStatus>>,
    pub output: Arc<RwLock<String>>,
    pub cost_usd: Arc<RwLock<Decimal>>,
    pub cancel: CancellationToken,
    pub finished: Arc<Notify>,
    /// Top-of-tree session — the user-facing root. ALL descendants
    /// (children, grandchildren) carry the same `root_session_id` so
    /// quota counters live in one bucket per root.
    pub root_session_id: SessionId,
}

#[derive(Debug, Error)]
pub enum SpawnError {
    #[error("subagent depth limit exceeded: requested {requested}, max {max}")]
    SubagentDepthExceeded { requested: u8, max: u8 },
    #[error("subagent quota exceeded: {in_flight} already in-flight, max {max}")]
    SubagentQuotaExceeded { in_flight: usize, max: usize },
    #[error("subagent type not found: {0}")]
    SubagentTypeNotFound(String),
    #[error("subagent spawn failed: {0}")]
    Internal(String),
}

impl SpawnError {
    /// Stable wire code surfaced as `code` in tool errors and the SSE
    /// `Error` event so integrators can branch on the failure class.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::SubagentDepthExceeded { .. } => "subagent_depth_exceeded",
            Self::SubagentQuotaExceeded { .. } => "subagent_quota_exceeded",
            Self::SubagentTypeNotFound(_) => "subagent_type_not_found",
            Self::Internal(_) => "subagent_internal_error",
        }
    }
}

/// Snapshot returned by `poll`. Cheap to construct; avoids leaking the
/// internal `Arc<RwLock<_>>` machinery across module boundaries.
#[derive(Debug, Clone)]
pub struct TaskSnapshot {
    pub task_id: TaskId,
    pub status: TaskStatus,
    pub output: String,
    pub cost_usd: Decimal,
    pub finished: bool,
}

/// Registry of running subagent tasks. Cloneable; interior mutability
/// only (DashMap + Arc).
#[derive(Default, Clone)]
pub struct TaskRegistry {
    tasks: Arc<DashMap<TaskId, TaskHandle>>,
    /// Quota counter per ROOT session. Every started task increments
    /// `session_descendants[root]`; completion / cancellation decrements.
    session_descendants: Arc<DashMap<SessionId, AtomicUsize>>,
    max_per_session: usize,
}

impl TaskRegistry {
    /// Construct a registry with the supplied quota cap. Tests pass a
    /// small number; production reads `OPENLET_SUBAGENT_MAX_PER_SESSION`
    /// via [`Self::from_env`].
    #[must_use]
    pub fn new(max_per_session: usize) -> Self {
        Self {
            tasks: Arc::new(DashMap::new()),
            session_descendants: Arc::new(DashMap::new()),
            max_per_session,
        }
    }

    /// Build a registry honoring the `OPENLET_SUBAGENT_MAX_PER_SESSION`
    /// env override (default [`DEFAULT_MAX_PER_SESSION`]).
    #[must_use]
    pub fn from_env() -> Self {
        let max = std::env::var("OPENLET_SUBAGENT_MAX_PER_SESSION")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_PER_SESSION);
        Self::new(max)
    }

    #[must_use]
    pub fn max_per_session(&self) -> usize {
        self.max_per_session
    }

    /// Pre-flight admission check. Increments the counter on success;
    /// caller MUST install the resulting `TaskHandle` via [`Self::insert`]
    /// or release via [`Self::release_quota`] on error.
    pub fn admit(&self, root: SessionId) -> Result<TaskId, SpawnError> {
        let entry = self
            .session_descendants
            .entry(root)
            .or_insert_with(|| AtomicUsize::new(0));
        // Unconditional fetch_add so two racing admits CAS deterministically.
        let prev = entry.fetch_add(1, Ordering::AcqRel);
        if prev >= self.max_per_session {
            entry.fetch_sub(1, Ordering::AcqRel);
            return Err(SpawnError::SubagentQuotaExceeded {
                in_flight: prev,
                max: self.max_per_session,
            });
        }
        Ok(TaskId::new())
    }

    /// Roll back a quota admit when spawning fails before the handle is
    /// installed (e.g. agent slug unknown).
    pub fn release_quota(&self, root: SessionId) {
        if let Some(c) = self.session_descendants.get(&root) {
            c.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// Install a `TaskHandle` keyed by `id`. Caller previously claimed
    /// the slot via [`Self::admit`]. Idempotent — the same id replaces
    /// an existing entry, but in practice ids are fresh UUIDs.
    pub fn insert(&self, id: TaskId, handle: TaskHandle) {
        self.tasks.insert(id, handle);
    }

    /// Drop a finished task from the live map and decrement its root's
    /// quota counter. Called from the spawned driver's Drop guard so the
    /// counter releases on success, error, OR panic.
    pub fn finalize(&self, id: TaskId) {
        if let Some((_, handle)) = self.tasks.remove(&id) {
            if let Some(c) = self.session_descendants.get(&handle.root_session_id) {
                c.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }

    #[must_use]
    pub fn poll(&self, id: TaskId) -> Option<TaskSnapshot> {
        let handle = self.tasks.get(&id)?.clone();
        let status = handle.status.blocking_read().clone();
        let output = handle.output.blocking_read().clone();
        let cost = *handle.cost_usd.blocking_read();
        let finished = status.is_terminal();
        Some(TaskSnapshot {
            task_id: id,
            status,
            output,
            cost_usd: cost,
            finished,
        })
    }

    /// Async-friendly poll. Prefer this from `.await` contexts; the sync
    /// [`Self::poll`] uses `blocking_read` and panics under a
    /// `current_thread` runtime if the lock is held by an async writer.
    pub async fn poll_async(&self, id: TaskId) -> Option<TaskSnapshot> {
        let handle = self.tasks.get(&id)?.clone();
        let status = handle.status.read().await.clone();
        let output = handle.output.read().await.clone();
        let cost = *handle.cost_usd.read().await;
        let finished = status.is_terminal();
        Some(TaskSnapshot {
            task_id: id,
            status,
            output,
            cost_usd: cost,
            finished,
        })
    }

    /// Trip the cancellation token for `id`. Idempotent.
    pub fn cancel(&self, id: TaskId) {
        if let Some(handle) = self.tasks.get(&id) {
            handle.cancel.cancel();
        }
    }

    /// Cascade cancel: every task whose root matches `root` is cancelled.
    /// Driver tasks observe their tokens, finalize, and release quota.
    pub fn cancel_descendants(&self, root: SessionId) {
        for entry in self.tasks.iter() {
            if entry.value().root_session_id == root {
                entry.value().cancel.cancel();
            }
        }
    }

    /// Park until task `id` finishes (status becomes terminal). Returns
    /// `None` if the task id was never installed or was already removed.
    pub async fn await_completion(&self, id: TaskId) -> Option<TaskSnapshot> {
        let handle = self.tasks.get(&id)?.clone();
        loop {
            let notified = handle.finished.notified();
            {
                let s = handle.status.read().await;
                if s.is_terminal() {
                    drop(s);
                    return self.poll_async(id).await;
                }
            }
            notified.await;
            // Re-check on wake — multiple notify_waiters may fire
            // before the status flips terminal.
        }
    }

    /// Append `delta` to the task's output buffer, capped at
    /// [`MAX_OUTPUT_BYTES`]. Once the cap trips, the buffer is replaced
    /// with `[truncated]` so subsequent appends remain bounded.
    pub async fn append_output(&self, id: TaskId, delta: &str) {
        let Some(handle) = self.tasks.get(&id) else {
            return;
        };
        let mut buf = handle.output.write().await;
        if buf.as_str() == "[truncated]" {
            return;
        }
        if buf.len().saturating_add(delta.len()) > MAX_OUTPUT_BYTES {
            *buf = "[truncated]".to_string();
            return;
        }
        buf.push_str(delta);
    }

    /// Replace the status atomically and signal `finished` waiters so
    /// `await_completion` can resume.
    pub async fn set_status(&self, id: TaskId, status: TaskStatus) {
        let Some(handle) = self.tasks.get(&id) else {
            return;
        };
        {
            let mut s = handle.status.write().await;
            *s = status;
        }
        handle.finished.notify_waiters();
    }

    /// Add `delta` to the task's accumulated cost. Used by the spawn
    /// driver on each provider-billed turn.
    pub async fn add_cost(&self, id: TaskId, delta: Decimal) {
        if let Some(handle) = self.tasks.get(&id) {
            let mut c = handle.cost_usd.write().await;
            *c += delta;
        }
    }

    /// Read-only handle clone (for testing).
    #[must_use]
    pub fn handle(&self, id: TaskId) -> Option<TaskHandle> {
        self.tasks.get(&id).map(|h| h.clone())
    }
}
