use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::message::MessageId;
use super::part::PartId;
use super::permission::{AskId, Decision, PermissionRequest};
use super::session::{SessionId, SessionStatus};
use crate::runtime::question_registry::QuestionId;

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
    /// `permission.resolved` — durable. `session_id` carried so per-session
    /// SSE replay queries return the resolution alongside the matching ask.
    PermissionResolved {
        session_id: SessionId,
        ask_id: AskId,
        decision: Decision,
    },
    /// `error` — durable.
    Error {
        session_id: Option<SessionId>,
        code: String,
        message: String,
    },
    /// `plugin.error` — durable. Emitted when a plugin hook panics,
    /// times out, or is denied at construction. Cloud users grep this
    /// to monitor plugin health without parsing structured logs.
    PluginError {
        session_id: Option<SessionId>,
        plugin_id: String,
        hook: String,
        message: String,
    },
    /// `question.requested` — durable. Emitted when an `ask_user` tool
    /// invocation suspends waiting for a frontend reply. The frontend
    /// observes this event and POSTs back to
    /// `/v1/sessions/:id/question/answer` with `question_id` + selected
    /// option indices.
    QuestionRequested {
        session_id: SessionId,
        question_id: QuestionId,
        header: String,
        question: String,
        options: Vec<AskOption>,
        multi_select: bool,
    },
    /// `plan_mode.entered` — durable. Fired by `EnterPlanMode` after the
    /// session's active agent slug is switched to the read-only `plan`
    /// profile. Subscribers (TUI banner, audit log) latch on this so
    /// they know to surface a "plan mode" indicator until the matching
    /// `PlanModeExited` arrives.
    PlanModeEntered {
        session_id: SessionId,
        at: DateTime<Utc>,
    },
    /// `plan_mode.exited` — durable. Carries the model's final plan
    /// text so subscribers can render it without re-reading message
    /// history. Emitted even when the session was not in plan mode
    /// (F2.6 — `ExitPlanMode` is a no-op-with-event so a naive model
    /// call still surfaces the plan to the operator).
    PlanModeExited {
        session_id: SessionId,
        plan: String,
        at: DateTime<Utc>,
    },
    /// `heartbeat` — TRANSIENT.
    Heartbeat,
}

/// One selectable option for an `ask_user` prompt. Rendered by the
/// frontend; the model receives the integer indices selected by the
/// user (single-select → exactly one; multi-select → zero or more).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
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
///
/// Fields cover the OpenAI-compat surface plus reasoning/cache breakdown
/// (cross-check §2/§5: opencode `session.ts:378-441` charges reasoning at
/// the output rate; OpenRouter returns `prompt_tokens_details.cached_tokens`
/// separately, so we keep `cached_input_tokens` distinct from
/// `input_tokens` to avoid double-counting on cost calc).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
}

/// Subscriber-side filter for `EventSink::subscribe`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventFilter {
    pub session_id: Option<SessionId>,
    pub include_transient: bool,
}
