//! Subagent task type definitions — extracted from `task_registry.rs`.
//!
//! Holds the public types every caller of `TaskRegistry` needs:
//! [`TaskId`], [`TaskStatus`], [`TaskHandle`], [`SpawnError`],
//! [`TaskSnapshot`], plus the bounded constants (output cap,
//! per-session quota, depth cap). The registry impl itself stays in
//! `task_registry.rs`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use thiserror::Error;
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::types::session::SessionId;

/// Per-task output cap — once a task's
/// concatenated output exceeds this, additional appends are discarded
/// and the buffer is replaced with a single `[truncated]` sentinel so
/// memory is bounded for adversarial subagent runs.
pub const MAX_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

/// Default per-root-session quota for in-flight subagent tasks.
/// Overridable via `LETI_SUBAGENT_MAX_PER_SESSION`.
pub const DEFAULT_MAX_PER_SESSION: usize = 32;

/// Default maximum nesting depth. Top-level user sessions are depth 0;
/// `subagent_task` calls increment by 1. Overridable via
/// `LETI_SUBAGENT_MAX_DEPTH`.
pub const DEFAULT_MAX_DEPTH: u8 = 3;

/// Default per-task inter-agent inbox depth cap (Phase 4). A sender that
/// floods a sibling past this many undrained messages is refused, bounding
/// memory against a chatty/adversarial peer.
pub const DEFAULT_MAX_INBOX_DEPTH: usize = 64;

/// Default per-message body cap (bytes) for inter-agent messages (Phase 4).
/// Distinct from inbox DEPTH — bounds a single oversized message, not just
/// the count (security Finding 2).
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Default per-root CUMULATIVE lifetime spawn budget — the total number of
/// subagents a single root session may ever spawn over its whole life
/// (distinct from the concurrency cap, which decrements on finalize). Set
/// generously so legitimate fan-out is never blocked; its job is only to
/// fail-close a runaway injection-driven spawn loop (Phase 3 Finding 15).
/// Overridable via `LETI_SUBAGENT_MAX_LIFETIME_SPAWNS`.
pub const DEFAULT_MAX_LIFETIME_SPAWNS: usize = 512;

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
    Interrupted,
    Failed(String),
}

/// Durable lifecycle status for a subagent execution. Unlike [`TaskStatus`],
/// this survives a process restart and deliberately distinguishes an
/// operator/process interruption from a terminal failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentExecutionStatus {
    Pending,
    Running,
    Finished,
    Failed,
    Cancelled,
    Interrupted,
}

impl SubagentExecutionStatus {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Finished | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "finished" => Self::Finished,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "interrupted" => Self::Interrupted,
            _ => return None,
        })
    }
}

/// Durable execution record. A child session is the agent's identity and
/// transcript; each invocation against it gets a fresh task id/record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubagentExecution {
    pub task_id: TaskId,
    pub root_session_id: SessionId,
    pub parent_session_id: SessionId,
    pub child_session_id: SessionId,
    pub agent_slug: String,
    pub objective: String,
    pub scope: Option<String>,
    pub background: bool,
    pub status: SubagentExecutionStatus,
    pub terminal_reason: Option<String>,
    pub output: String,
    pub cost_usd: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub version: u64,
}

/// The sole owner of a task's terminal output.  A foreground waiter may
/// hand ownership to the background outbox while the child is still running,
/// but settlement can only choose one of the two terminal states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOwnership {
    ForegroundWaiting,
    Background,
    TerminalForeground,
    TerminalBackground,
}

impl DeliveryOwnership {
    pub(crate) const fn as_u8(self) -> u8 {
        match self {
            Self::ForegroundWaiting => 0,
            Self::Background => 1,
            Self::TerminalForeground => 2,
            Self::TerminalBackground => 3,
        }
    }

    pub(crate) const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Background,
            2 => Self::TerminalForeground,
            3 => Self::TerminalBackground,
            _ => Self::ForegroundWaiting,
        }
    }
}

impl TaskStatus {
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Finished | Self::Cancelled | Self::Interrupted | Self::Failed(_)
        )
    }

    /// Stable wire label. Used by `task_status` tool + SSE.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Finished => "finished",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
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
    /// Session that issued the task. Used to authorize a TUI conversion.
    pub parent_session_id: SessionId,
    /// Linearizable foreground/background output ownership state.
    pub delivery: Arc<AtomicU8>,
    /// One-shot guard for the TERMINAL SIDE-EFFECT (the `SubagentSettled`
    /// publish and any future result injection) — NOT the quota
    /// decrement. A background task settles
    /// via a second path; without this guard it could surface its result
    /// twice (once via the normal terminal publish, once via injection).
    /// `claim_settle` flips it exactly once so the terminal side-effect
    /// fires a single time. The quota `finalize`/`saturating_dec` path is
    /// deliberately left ungated — see `task_registry::saturating_dec`.
    pub settled: Arc<AtomicBool>,
    /// Wake signal for the re-armable driver loop (Phase 2). A subagent
    /// that stays alive in the background with a
    /// live inbox — Phase 4) parks on this between `run_loop` objectives;
    /// an external event (message arrival, resume) notifies it to re-enter
    /// `run_loop`. A plain sync child never parks (its loop breaks on first
    /// exit), so this is inert for the Phase 1 single-shot path.
    /// Enabled-before-check at the wait site to avoid the lost-wakeup bug
    /// the registry already documents for `finished`.
    pub inbox_notify: Arc<Notify>,
    /// Inter-agent message inbox (Phase 4). A sibling's `send_message`
    /// pushes an [`InboxMessage`] here; the re-armable driver loop drains
    /// it at the top of the next iteration and projects each message as an
    /// untrusted `SiblingMessage`-origin turn. Bounded in BOTH depth
    /// (count) and per-message length by the registry's `push_message`, so
    /// a chatty or adversarial sender can't exhaust memory. Dropped with
    /// the handle on teardown — no separate lifecycle to leak.
    pub inbox: Arc<Mutex<VecDeque<InboxMessage>>>,
}

/// A single queued inter-agent message (Phase 4). `from` is a stable
/// sender-provenance label (currently `session:<uuid>`) surfaced in the
/// receiver's untrusted-data framing as `from=...`. It is set by the
/// runtime, never by the message body, so a malicious body cannot spoof a
/// trusted sender identity.
#[derive(Debug, Clone)]
pub struct InboxMessage {
    /// Durable inbox row id when delivery came through `MemoryStore`.
    pub id: Option<String>,
    pub from: String,
    pub body: String,
}

/// Unique roster handle for an addressable live sibling. A bare agent slug
/// collides when two same-type siblings run at once (two `reviewer`s), so
/// the roster disambiguates with a suffix (`reviewer`, `reviewer#2`). This
/// newtype keeps the "unique name" contract legible at call sites.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HandleName(pub String);

impl std::fmt::Display for HandleName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A live sibling's roster record (Phase 4). Keyed by [`HandleName`] under
/// a root session. `gen` is bumped whenever a name is (re)bound so a
/// `send_message` holding a stale snapshot is refused rather than
/// misrouted to whatever task now holds the name (Claude Code v2.199
/// name-safety generation check). `parent` scopes reachability to
/// same-parent siblings by default (hierarchy containment). `allowlist`
/// is the receiver's tool allowlist, consulted by the privilege check so a
/// low-privilege sender can't escalate by messaging a high-privilege peer.
#[derive(Debug, Clone)]
pub struct RosterEntry {
    pub task_id: TaskId,
    pub generation: u64,
    pub parent: SessionId,
    pub allowlist: Arc<[String]>,
}

impl TaskHandle {
    /// Attempt to claim the one-shot terminal side-effect slot. Returns
    /// `true` for the FIRST caller (which then owns publishing the
    /// terminal `SubagentSettled` / injecting the result); every
    /// subsequent caller gets `false` and must skip the side-effect. This
    /// guards ONLY the terminal side-effect — the quota decrement in
    /// `finalize` stays idempotent via `saturating_dec` and is never
    /// gated by this flag (guarding the decrement would invert the safe
    /// no-op into a permanent slot leak; see Phase 1 Finding 13).
    #[must_use]
    pub fn claim_settle(&self) -> bool {
        self.settled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

#[derive(Debug, Error)]
pub enum SpawnError {
    #[error("subagent depth limit exceeded: requested {requested}, max {max}")]
    SubagentDepthExceeded { requested: u8, max: u8 },
    #[error("subagent quota exceeded: {in_flight} already in-flight, max {max}")]
    SubagentQuotaExceeded { in_flight: usize, max: usize },
    #[error("subagent lifetime spawn budget exceeded: {spawned} spawned, max {max}")]
    SubagentLifetimeBudgetExceeded { spawned: usize, max: usize },
    #[error("subagent type not found: {0}")]
    SubagentTypeNotFound(String),
    /// An inter-agent `send_message` was refused (Phase 4): unknown /
    /// finalized target, over-length body, full inbox, stale name
    /// generation, privilege escalation, or out-of-scope reachability.
    #[error("message rejected: {0}")]
    MessageRejected(String),
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
            Self::SubagentLifetimeBudgetExceeded { .. } => "subagent_lifetime_budget_exceeded",
            Self::SubagentTypeNotFound(_) => "subagent_type_not_found",
            Self::MessageRejected(_) => "message_rejected",
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
