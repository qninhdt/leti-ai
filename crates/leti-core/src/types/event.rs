use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::message::MessageId;
use super::part::PartId;
use super::permission::{AskId, Decision, PermissionRequest};
use super::question::QuestionId;
use super::session::{InteractionMode, SessionId, SessionStatus};

/// Domain event published on the bus and (depending on `Persistence`)
/// persisted to SQLite via the two-tier publisher.
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
    /// `permission.detached_authorized` — durable audit record for every
    /// permission check in a detached session, including direct Allows and
    /// explicit Denies that never create a Pending ask.
    DetachedToolAuthorized {
        session_id: SessionId,
        tool: String,
        request: PermissionRequest,
        decision: Decision,
        interaction_mode: InteractionMode,
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
    /// `/v1/session/:id/question/answer` with `question_id` + selected
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
    /// (`ExitPlanMode` is a no-op-with-event so a naive model
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
    /// `subagent.spawned` — durable. Emitted when the in-process
    /// `subagent_task` tool admits a new descendant task. Carries the
    /// PARENT session id so SSE consumers tracking the parent see the
    /// fan-out without subscribing globally.
    SubagentSpawned {
        task_id: Uuid,
        tool_call_id: String,
        child_session_id: SessionId,
        parent_session_id: SessionId,
        subagent_type: String,
        objective: String,
        description: Option<String>,
        background: bool,
    },
    /// `subagent.progress` — TRANSIENT. Streaming text fragment from a
    /// running subagent's assistant turn. Bounded by the per-task 10MB
    /// output cap (see `runtime::subagent::task_registry::MAX_OUTPUT_BYTES`).
    /// `parent_session_id` lets per-session SSE subscribers see child
    /// progress without a global subscription.
    SubagentProgress {
        task_id: Uuid,
        parent_session_id: SessionId,
        delta: String,
    },
    /// `subagent.settled` — durable lifecycle metadata. Child output never
    /// rides public SSE; foreground output belongs to the original tool
    /// result and background output belongs to its typed parent reminder.
    SubagentSettled {
        task_id: Uuid,
        child_session_id: SessionId,
        parent_session_id: SessionId,
        status: String,
        cost_usd: Option<String>,
    },
    /// `subagent.message` — durable. Emitted when a sibling subagent sends
    /// an inter-agent message via `send_message` (Phase 4). Carries the
    /// sender + receiver unique handle names and the receiver's `task_id`.
    /// `parent_session_id` routes it to the shared parent's SSE stream so
    /// the TUI task panel can show cross-sibling activity. The message
    /// BODY is intentionally NOT on the wire — it is delivered in-band as
    /// an untrusted injected turn; the frame is activity metadata only.
    SubagentMessage {
        task_id: Uuid,
        parent_session_id: SessionId,
        from: String,
        to: String,
    },
    /// `subagent.roster` — durable. Emitted on any roster change (a
    /// sibling registered / removed). Carries the live named siblings for
    /// a root so the TUI `@mention` typeahead has a data source (Phase 4
    /// Finding 11). `entries` is `(name, task_id, gen)` sorted by name.
    SubagentRoster {
        root_session_id: SessionId,
        entries: Vec<RosterFrameEntry>,
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
    /// `todo.updated` — durable. Emitted by the `todo` tool after a
    /// confirmed (atomic) persist of the session's checklist. Carries the
    /// full item list so the TUI re-renders the checklist live without
    /// re-reading the artifact. The full-overwrite semantics of `todo`
    /// mean each event is the authoritative current list.
    TodoUpdated {
        session_id: SessionId,
        items: Vec<TodoEventItem>,
    },
    /// `heartbeat` — TRANSIENT.
    Heartbeat,
}

impl AgentEvent {
    /// Stable wire discriminator (the SSE `event:` name / persisted kind).
    /// Single source of truth so the SSE encoder and the event repo can
    /// never disagree on the string for a variant.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SessionStatus { .. } => "session.status",
            Self::MessageCreated { .. } => "message.created",
            Self::PartCreated { .. } => "part.created",
            Self::PartDelta { .. } => "part.delta",
            Self::PartUpdated { .. } => "part.updated",
            Self::StepFinished { .. } => "step.finished",
            Self::PermissionAsked { .. } => "permission.asked",
            Self::PermissionResolved { .. } => "permission.resolved",
            Self::DetachedToolAuthorized { .. } => "permission.detached_authorized",
            Self::Error { .. } => "error",
            Self::PluginError { .. } => "plugin.error",
            Self::QuestionRequested { .. } => "question.requested",
            Self::PlanModeEntered { .. } => "plan_mode.entered",
            Self::PlanModeExited { .. } => "plan_mode.exited",
            Self::AttachmentAccepted { .. } => "attachment.accepted",
            Self::SubagentSpawned { .. } => "subagent.spawned",
            Self::SubagentProgress { .. } => "subagent.progress",
            Self::SubagentSettled { .. } => "subagent.settled",
            Self::SubagentMessage { .. } => "subagent.message",
            Self::SubagentRoster { .. } => "subagent.roster",
            Self::NotificationEmitted { .. } => "notification.emitted",
            Self::TodoUpdated { .. } => "todo.updated",
            Self::Heartbeat => "heartbeat",
        }
    }

    /// The session this event belongs to, if any. Subagent events carry
    /// the PARENT session so per-session SSE subscribers see child
    /// fan-out; `Error`/`PluginError`/`NotificationEmitted` are
    /// session-optional; `Heartbeat` has none.
    #[must_use]
    pub fn session_id(&self) -> Option<SessionId> {
        match self {
            Self::SessionStatus { session_id, .. }
            | Self::MessageCreated { session_id, .. }
            | Self::PartCreated { session_id, .. }
            | Self::PartDelta { session_id, .. }
            | Self::PartUpdated { session_id, .. }
            | Self::StepFinished { session_id, .. }
            | Self::PermissionAsked { session_id, .. }
            | Self::PermissionResolved { session_id, .. }
            | Self::DetachedToolAuthorized { session_id, .. }
            | Self::QuestionRequested { session_id, .. }
            | Self::PlanModeEntered { session_id, .. }
            | Self::PlanModeExited { session_id, .. }
            | Self::AttachmentAccepted { session_id, .. }
            | Self::TodoUpdated { session_id, .. } => Some(*session_id),
            Self::Error { session_id, .. }
            | Self::PluginError { session_id, .. }
            | Self::NotificationEmitted { session_id, .. } => *session_id,
            Self::SubagentSpawned {
                parent_session_id, ..
            }
            | Self::SubagentProgress {
                parent_session_id, ..
            }
            | Self::SubagentSettled {
                parent_session_id, ..
            }
            | Self::SubagentMessage {
                parent_session_id, ..
            } => Some(*parent_session_id),
            Self::SubagentRoster {
                root_session_id, ..
            } => Some(*root_session_id),
            Self::Heartbeat => None,
        }
    }
}

/// One checklist item carried on a `todo.updated` event. Kept as a
/// self-contained struct with string fields (rather than referencing the
/// `todo` tool's enums) so the IO-free `types/` layer does not depend on
/// the `tools/` layer. `status` / `priority` carry the same snake_case
/// wire strings the tool itself emits (`pending` / `in_progress` /
/// `completed`, `high` / `medium` / `low`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoEventItem {
    pub content: String,
    pub status: String,
    pub priority: String,
}

/// One live sibling in a `subagent.roster` frame. Carries only what the
/// TUI @mention typeahead needs: the unique handle name, the task id it
/// resolves to, and the generation (so a UI keyed on `{name, gen}`
/// replaces a stale entry when a name is rebound).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterFrameEntry {
    pub name: String,
    pub task_id: Uuid,
    pub generation: u64,
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

/// Severity for `NotificationEmitted`. Canonical definition — kept here
/// (IO-free `types/`) so adapters / DTOs depend on `event.rs` only.
/// [`crate::hooks::io`] re-exports this for the plugin hook API.
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
/// (reasoning is charged at the output rate; OpenRouter returns
/// `prompt_tokens_details.cached_tokens`
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
    /// Authoritative turn cost in USD as reported by the gateway
    /// (OpenRouter sends `usage.cost` when `stream_options.include_usage`
    /// is set). `None` when the gateway omits it. Preferred over the
    /// static pricing table so cost is correct for every model — incl.
    /// ones with no local pricing row — and reflects real billing
    /// (BYOK / discounts). Skipped on the wire when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<Decimal>,
}

/// Subscriber-side filter for `EventSink::subscribe`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventFilter {
    pub session_id: Option<SessionId>,
    pub include_transient: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::session::SessionId;

    /// Locks the wire discriminators. The SSE encoder and the SQLite
    /// event repo both depend on these exact strings; a silent rename
    /// would desync persisted rows from live frames. If a variant is
    /// added, extend this list deliberately.
    #[test]
    fn kind_strings_are_stable() {
        let sid = SessionId::new();
        let cases: &[(AgentEvent, &str)] = &[
            (
                AgentEvent::Error {
                    session_id: Some(sid),
                    code: "x".into(),
                    message: "y".into(),
                },
                "error",
            ),
            (
                AgentEvent::PluginError {
                    session_id: None,
                    plugin_id: "p".into(),
                    hook: "h".into(),
                    message: "m".into(),
                },
                "plugin.error",
            ),
            (AgentEvent::Heartbeat, "heartbeat"),
        ];
        for (ev, want) in cases {
            assert_eq!(ev.kind(), *want);
        }
    }

    /// Subagent events report the PARENT session; `Heartbeat` has none.
    #[test]
    fn session_id_routing() {
        let parent = SessionId::new();
        let spawned = AgentEvent::SubagentSpawned {
            task_id: uuid::Uuid::nil(),
            tool_call_id: "call".into(),
            child_session_id: SessionId::new(),
            parent_session_id: parent,
            subagent_type: "t".into(),
            objective: "task".into(),
            description: None,
            background: false,
        };
        assert_eq!(spawned.session_id(), Some(parent));
        assert_eq!(AgentEvent::Heartbeat.session_id(), None);
    }
}
