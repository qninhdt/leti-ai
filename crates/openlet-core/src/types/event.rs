use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    /// `attachment.accepted` — durable. Emitted by the multipart upload
    /// route after the bytes are persisted to `ArtifactStore` and the
    /// matching `Part::Image` / `Part::Document` is appended to the
    /// session. Frontends render an attachment chip without re-fetching
    /// the message log. `summary` is human-readable (e.g. "PNG 1024x768"
    /// or "PDF, 12 pages, 3.4kB extracted text") — never includes
    /// extracted text body so audit redactor doesn't have to scan it.
    AttachmentAccepted {
        session_id: SessionId,
        message_id: MessageId,
        part_id: PartId,
        artifact_id: String,
        attachment_kind: AttachmentKind,
        mime: String,
        summary: String,
    },
    /// `subagent.started` — durable. Emitted when the in-process
    /// `subagent_task` tool admits a new descendant task. Carries the
    /// PARENT session id so SSE consumers tracking the parent see the
    /// fan-out without subscribing globally.
    SubagentStarted {
        task_id: Uuid,
        parent_session_id: SessionId,
        subagent_type: String,
    },
    /// `subagent.output` — TRANSIENT. Streaming text fragment from a
    /// running subagent's assistant turn. Bounded by the per-task 10MB
    /// output cap (see `runtime::subagent::task_registry::MAX_OUTPUT_BYTES`).
    SubagentOutput { task_id: Uuid, delta: String },
    /// `subagent.finished` — durable. Carries final output snapshot +
    /// cost so a parent's `task_status` poll observes a consistent
    /// terminal state.
    SubagentFinished {
        task_id: Uuid,
        output: String,
        cost_usd: Option<String>,
    },
    /// `notification.emitted` — durable. Plugin-emitted user-facing
    /// notification. `body` has been redacted by the secret redactor
    /// before publish. Per-session rate-limit drops surplus emits and
    /// emits a tracing warn instead.
    NotificationEmitted {
        session_id: Option<SessionId>,
        level: NotificationLevel,
        title: String,
        body: String,
        plugin_id: String,
    },
    /// `heartbeat` — TRANSIENT.
    Heartbeat,
}

/// Discriminator for `AttachmentAccepted`. Mirrors the upload route's
/// content-sniff result so the frontend doesn't have to re-classify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    Document,
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

/// Severity for `NotificationEmitted`. Wire mirror of
/// [`crate::hooks::io::NotificationLevel`] — kept here so adapters /
/// DTOs depend on `event.rs` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevel {
    #[default]
    Info,
    Warn,
    Error,
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
    /// Read-side cached prompt tokens. OpenRouter / Anthropic /
    /// DashScope all expose a "this prompt was served from cache"
    /// counter — keep it distinct from `input_tokens` so cost calc
    /// doesn't double-charge.
    pub cached_input_tokens: u64,
    /// Tokens written into a fresh cache slot during this turn.
    /// Anthropic ephemeral cache and DashScope context cache both
    /// charge for cache writes at a higher rate than reads.
    #[serde(default)]
    pub cache_write_tokens: u64,
    /// Alias for `cache_write_tokens` matching the Anthropic Messages
    /// API field name. Kept additive so plugins that already write
    /// `cache_write_tokens` still work; one of the two is sufficient.
    /// Cost math sums both: a turn that populates EITHER field gets
    /// charged once at the cache_write rate.
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
}

/// Subscriber-side filter for `EventSink::subscribe`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventFilter {
    pub session_id: Option<SessionId>,
    pub include_transient: bool,
}
