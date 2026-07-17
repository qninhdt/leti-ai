//! Shared encode/decode helpers for the SQLite adapters.
//!
//! Centralizes the small, repetitive bits scattered across
//! [`memory_store`](super::memory_store), [`event_repo`](super::event_repo),
//! and [`permission_repo`](super::permission_repo):
//!
//!  - `sqlx::Error` → `MemoryError::Io(...)` mapping
//!  - millisecond timestamp conversion (`now_ms`, `from_ms`)
//!  - UUID parsing wrappers (`parse_uuid`)
//!  - JSON encode/decode wrappers with contextual error messages
//!  - enum ↔ string mapping for `Role`, `SessionStatus`, `PermissionMode`,
//!    plus the `part_kind` discriminator

use chrono::{DateTime, TimeZone, Utc};
use serde::{Serialize, de::DeserializeOwned};
use uuid::Uuid;

use leti_core::error::MemoryError;
use leti_core::types::message::Role;
use leti_core::types::part::Part;
use leti_core::types::permission::PermissionMode;
use leti_core::types::session::{DetachedAsk, InteractionMode, SessionStatus};

/// Convert a `sqlx::Error` into a `MemoryError::Io` carrying the
/// driver's textual representation. Used at every sqlx call site so
/// the row mappers stay one-liner-friendly.
#[inline]
pub(crate) fn map_io(e: sqlx::Error) -> MemoryError {
    MemoryError::Io(e.to_string())
}

/// Wall-clock "now" as ms since unix epoch — the storage format we
/// use for every `created_at` / `updated_at` column.
#[inline]
pub(crate) fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Inverse of [`now_ms`]: ms-since-epoch back to UTC `DateTime`.
/// Falls back to "now" when the value is out of range — keeps the
/// row mapper infallible since we already trust DB-stored timestamps.
#[inline]
pub(crate) fn from_ms(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now)
}

/// Parse a UUID string from a DB column into a `Uuid`. Returns
/// `MemoryError::Io` with a contextual message on parse failure.
#[inline]
pub(crate) fn parse_uuid(s: &str) -> Result<Uuid, MemoryError> {
    Uuid::parse_str(s).map_err(|e| MemoryError::Io(format!("uuid parse: {e}")))
}

/// Serialize a value to JSON for storage. `ctx` is used to label the
/// error so callers can tell which field failed.
#[inline]
pub(crate) fn encode_json<T: Serialize>(t: &T, ctx: &str) -> Result<String, MemoryError> {
    serde_json::to_string(t).map_err(|e| MemoryError::Io(format!("{ctx}: {e}")))
}

/// Inverse of [`encode_json`]. Same contextual-error pattern.
#[inline]
pub(crate) fn decode_json<T: DeserializeOwned>(s: &str, ctx: &str) -> Result<T, MemoryError> {
    serde_json::from_str(s).map_err(|e| MemoryError::Io(format!("{ctx}: {e}")))
}

#[inline]
pub(crate) fn role_str(r: Role) -> &'static str {
    match r {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

pub(crate) fn parse_role(s: &str) -> Result<Role, MemoryError> {
    match s {
        "system" => Ok(Role::System),
        "user" => Ok(Role::User),
        "assistant" => Ok(Role::Assistant),
        "tool" => Ok(Role::Tool),
        other => Err(MemoryError::Io(format!("unknown role: {other}"))),
    }
}

#[inline]
pub(crate) fn status_str(s: SessionStatus) -> &'static str {
    match s {
        SessionStatus::Idle => "idle",
        SessionStatus::Running => "running",
        SessionStatus::Cancelling => "cancelling",
        SessionStatus::Cancelled => "cancelled",
        SessionStatus::Errored => "errored",
    }
}

pub(crate) fn parse_status(s: &str) -> Result<SessionStatus, MemoryError> {
    match s {
        "idle" => Ok(SessionStatus::Idle),
        "running" => Ok(SessionStatus::Running),
        "cancelling" => Ok(SessionStatus::Cancelling),
        "cancelled" => Ok(SessionStatus::Cancelled),
        "errored" => Ok(SessionStatus::Errored),
        other => Err(MemoryError::Io(format!("unknown status: {other}"))),
    }
}

#[inline]
pub(crate) fn mode_str(m: PermissionMode) -> &'static str {
    match m {
        PermissionMode::ReadOnly => "read_only",
        PermissionMode::WorkspaceWrite => "workspace_write",
        PermissionMode::Danger => "danger",
    }
}

pub(crate) fn parse_mode(s: &str) -> Result<PermissionMode, MemoryError> {
    match s {
        "read_only" => Ok(PermissionMode::ReadOnly),
        "workspace_write" => Ok(PermissionMode::WorkspaceWrite),
        "danger" => Ok(PermissionMode::Danger),
        other => Err(MemoryError::Io(format!("unknown mode: {other}"))),
    }
}

#[inline]
pub(crate) fn interaction_mode_parts(
    mode: InteractionMode,
) -> (&'static str, Option<&'static str>) {
    match mode {
        InteractionMode::Interactive => ("interactive", None),
        InteractionMode::Detached {
            on_ask: DetachedAsk::Allow,
        } => ("detached", Some("allow")),
        InteractionMode::Detached {
            on_ask: DetachedAsk::Deny,
        } => ("detached", Some("deny")),
    }
}

pub(crate) fn parse_interaction_mode(
    mode: &str,
    on_ask: Option<&str>,
) -> Result<InteractionMode, MemoryError> {
    match mode {
        "interactive" => Ok(InteractionMode::Interactive),
        "detached" => match on_ask.unwrap_or("deny") {
            "allow" => Ok(InteractionMode::Detached {
                on_ask: DetachedAsk::Allow,
            }),
            "deny" => Ok(InteractionMode::Detached {
                on_ask: DetachedAsk::Deny,
            }),
            other => Err(MemoryError::Io(format!("unknown detached on_ask: {other}"))),
        },
        other => Err(MemoryError::Io(format!(
            "unknown interaction mode: {other}"
        ))),
    }
}

/// Discriminator string stored in `parts.kind`. Stable wire identifier
/// — not derived from `Part`'s serde tag because that may change.
#[inline]
pub(crate) fn part_kind(part: &Part) -> &'static str {
    match part {
        Part::Text { .. } => "text",
        Part::Reasoning { .. } => "reasoning",
        Part::ToolCall { .. } => "tool_call",
        Part::ToolResult { .. } => "tool_result",
        Part::Image { .. } => "image",
        Part::Document { .. } => "document",
        Part::StepStart { .. } => "step_start",
        Part::StepFinish { .. } => "step_finish",
        Part::Compaction { .. } => "compaction",
        Part::CompactionRequest { .. } => "compaction_request",
        Part::Plan { .. } => "plan",
        Part::RuntimeReminder { .. } => "runtime_reminder",
    }
}
