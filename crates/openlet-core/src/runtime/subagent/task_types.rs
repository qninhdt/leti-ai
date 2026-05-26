//! Subagent task type definitions — extracted from `task_registry.rs`.
//!
//! Holds the public types every caller of `TaskRegistry` needs:
//! [`TaskId`], [`TaskStatus`], [`TaskHandle`], [`SpawnError`],
//! [`TaskSnapshot`], plus the bounded constants (output cap,
//! per-session quota, depth cap). The registry impl itself stays in
//! `task_registry.rs`.

use std::sync::Arc;

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
