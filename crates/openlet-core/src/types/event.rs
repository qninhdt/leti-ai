use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::message::MessageId;
use super::part::PartId;
use super::permission::{AskId, Decision, PermissionRequest};
use super::session::{SessionId, SessionStatus};

/// Domain event published on the bus and (depending on `Persistence`)
/// persisted to SQLite. Phase 5 wires the two-tier publisher (§G).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
    /// `session.status` — durable.
    SessionStatus {
        session_id: SessionId,
        status: SessionStatus,
        at: DateTime<Utc>,
    },
    /// `message.created` — durable.
    MessageCreated {
        session_id: SessionId,
        message_id: MessageId,
        at: DateTime<Utc>,
    },
    /// `part.created` — durable.
    PartCreated {
        session_id: SessionId,
        message_id: MessageId,
        part_id: PartId,
        at: DateTime<Utc>,
    },
    /// `part.delta` — TRANSIENT (broadcast only, not persisted).
    PartDelta {
        session_id: SessionId,
        message_id: MessageId,
        part_id: PartId,
        delta_kind: DeltaKind,
        delta: String,
    },
    /// `part.updated` — durable. State transitions only, NOT text deltas.
    PartUpdated {
        session_id: SessionId,
        message_id: MessageId,
        part_id: PartId,
    },
    /// `step.finished` — durable. Carries usage + cost.
    StepFinished {
        session_id: SessionId,
        message_id: MessageId,
        reason: String,
        usage: Option<Usage>,
        cost_decimal_str: Option<String>,
    },
    /// `permission.asked` — durable.
    PermissionAsked {
        session_id: SessionId,
        ask_id: AskId,
        request: PermissionRequest,
    },
    /// `permission.resolved` — durable.
    PermissionResolved {
        ask_id: AskId,
        decision: Decision,
    },
    /// `error` — durable.
    Error {
        session_id: Option<SessionId>,
        code: String,
        message: String,
    },
    /// `heartbeat` — TRANSIENT.
    Heartbeat,
}

/// Streaming delta kind (`part.delta` payload discriminator).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaKind {
    Text,
    Reasoning,
    ToolArgs,
}

/// Token + cost telemetry attached to `StepFinished` events.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
}

/// Subscriber-side filter for `EventSink::subscribe`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventFilter {
    pub session_id: Option<SessionId>,
    pub include_transient: bool,
}
