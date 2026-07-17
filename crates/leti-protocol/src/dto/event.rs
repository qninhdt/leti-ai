//! Event DTOs for SSE frames.
//!
//! Wire shape mirrors `leti_core::types::event::AgentEvent` 1:1; we
//! re-derive `ToSchema` here so utoipa picks the type up. Field
//! ordering matches the durable JSON we already write to the `events`
//! table — no double-translation cost.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use leti_core::types::event::{
    AgentEvent, AskOption, AttachmentKind, DeltaKind, NotificationLevel, TodoEventItem, Usage,
};
use leti_core::types::session::SessionStatus;

use super::permission::PermissionRequestDto;

/// SSE-encoded frame envelope. Server emits `id:` (events.id) +
/// `event:` (kind) + `data:` (`AgentEvent` JSON).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventDto {
    SessionStatus {
        session_id: Uuid,
        status: SessionStatus,
        at: DateTime<Utc>,
    },
    MessageCreated {
        session_id: Uuid,
        message_id: Uuid,
        at: DateTime<Utc>,
    },
    PartCreated {
        session_id: Uuid,
        message_id: Uuid,
        part_id: Uuid,
        at: DateTime<Utc>,
    },
    PartDelta {
        session_id: Uuid,
        message_id: Uuid,
        part_id: Uuid,
        delta_kind: DeltaKindDto,
        delta: String,
    },
    PartUpdated {
        session_id: Uuid,
        message_id: Uuid,
        part_id: Uuid,
    },
    StepFinished {
        session_id: Uuid,
        message_id: Uuid,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<UsageDto>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_decimal_str: Option<String>,
    },
    PermissionAsked {
        session_id: Uuid,
        request: PermissionRequestDto,
    },
    PermissionResolved {
        session_id: Uuid,
        ask_id: Uuid,
        decision: PermissionDecisionDto,
    },
    DetachedToolAuthorized {
        session_id: Uuid,
        tool: String,
        request: PermissionRequestDto,
        decision: PermissionDecisionDto,
    },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<Uuid>,
        code: String,
        message: String,
    },
    PluginError {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<Uuid>,
        plugin_id: String,
        hook: String,
        message: String,
    },
    QuestionRequested {
        session_id: Uuid,
        question_id: Uuid,
        header: String,
        question: String,
        options: Vec<AskOptionDto>,
        multi_select: bool,
    },
    PlanModeEntered {
        session_id: Uuid,
        at: DateTime<Utc>,
    },
    PlanModeExited {
        session_id: Uuid,
        plan: String,
        at: DateTime<Utc>,
    },
    AttachmentAccepted {
        session_id: Uuid,
        message_id: Uuid,
        part_id: Uuid,
        artifact_id: String,
        attachment_kind: AttachmentKindDto,
        mime: String,
        summary: String,
    },
    SubagentSpawned {
        task_id: Uuid,
        tool_call_id: String,
        child_session_id: Uuid,
        parent_session_id: Uuid,
        subagent_type: String,
        objective: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        description: Option<String>,
        background: bool,
    },
    SubagentProgress {
        task_id: Uuid,
        parent_session_id: Uuid,
        delta: String,
    },
    SubagentSettled {
        task_id: Uuid,
        child_session_id: Uuid,
        parent_session_id: Uuid,
        status: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_usd: Option<String>,
    },
    SubagentMessage {
        task_id: Uuid,
        parent_session_id: Uuid,
        from: String,
        to: String,
    },
    SubagentRoster {
        root_session_id: Uuid,
        entries: Vec<RosterEntryDto>,
    },
    NotificationEmitted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<Uuid>,
        level: NotificationLevelDto,
        title: String,
        body: String,
        plugin_id: String,
    },
    TodoUpdated {
        session_id: Uuid,
        items: Vec<TodoItemDto>,
    },
    Heartbeat,
}

/// Wire shape for a `subagent.roster` entry. Mirrors
/// `leti_core::types::event::RosterFrameEntry`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RosterEntryDto {
    pub name: String,
    pub task_id: Uuid,
    pub generation: u64,
}

/// Wire shape for a `todo.updated` item. Mirrors
/// `leti_core::types::event::TodoEventItem` 1:1 (string fields).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TodoItemDto {
    pub content: String,
    pub status: String,
    pub priority: String,
}

impl From<TodoEventItem> for TodoItemDto {
    fn from(i: TodoEventItem) -> Self {
        Self {
            content: i.content,
            status: i.status,
            priority: i.priority,
        }
    }
}

/// Wire shape for `AttachmentKind`. Mirrors the domain enum 1:1.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKindDto {
    Image,
    Document,
}

impl From<AttachmentKind> for AttachmentKindDto {
    fn from(k: AttachmentKind) -> Self {
        match k {
            AttachmentKind::Image => AttachmentKindDto::Image,
            AttachmentKind::Document => AttachmentKindDto::Document,
        }
    }
}

/// Wire shape for an `ask_user` option.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AskOptionDto {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl From<AskOption> for AskOptionDto {
    fn from(o: AskOption) -> Self {
        Self {
            label: o.label,
            description: o.description,
        }
    }
}

/// Wire mirror of [`leti_core::types::event::NotificationLevel`].
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotificationLevelDto {
    #[default]
    Info,
    Warn,
    Error,
}

impl From<NotificationLevel> for NotificationLevelDto {
    fn from(l: NotificationLevel) -> Self {
        match l {
            NotificationLevel::Info => NotificationLevelDto::Info,
            NotificationLevel::Warn => NotificationLevelDto::Warn,
            NotificationLevel::Error => NotificationLevelDto::Error,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeltaKindDto {
    Text,
    Reasoning,
    ToolArgs,
}

impl From<DeltaKind> for DeltaKindDto {
    fn from(k: DeltaKind) -> Self {
        match k {
            DeltaKind::Text => DeltaKindDto::Text,
            DeltaKind::Reasoning => DeltaKindDto::Reasoning,
            DeltaKind::ToolArgs => DeltaKindDto::ToolArgs,
        }
    }
}

/// Wire-shape token usage summary.
///
/// Lossy conversion notes:
/// - `cache_write_tokens` is the **sum** of the domain type's
///   `cache_write_tokens` + `cache_creation_input_tokens`. Both represent
///   cache-write billing; combining them gives consumers a single number
///   matching what was actually charged.
/// - `cost_usd` from the domain `Usage` is intentionally dropped here.
///   Per-step cost surfaces via `StepFinished.cost_decimal_str` instead,
///   keeping this struct purely about token counts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct UsageDto {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
}

/// Resolved-permission outcome on the wire. Mirrors `Decision`'s
/// `tag = "outcome"` serde shape so allow / deny / pending remain
/// distinguishable, AND `Deny.feedback` reaches the SSE consumer
/// (previously collapsed to a bare `"deny"` label).
///
/// Note: the `Pending` variant is technically unreachable in
/// `PermissionResolved` events — those only fire after a decision is
/// made. It exists here for exhaustive mapping of the domain enum and
/// to avoid a `panic!` / `unreachable!()` in the conversion path.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PermissionDecisionDto {
    Allow,
    Deny {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feedback: Option<String>,
    },
    Pending {
        ask_id: Uuid,
    },
}

impl PermissionDecisionDto {
    #[must_use]
    pub fn from_decision(d: &leti_core::types::permission::Decision) -> Self {
        use leti_core::types::permission::Decision;
        match d {
            Decision::Allow => Self::Allow,
            Decision::Deny { feedback } => Self::Deny {
                feedback: feedback.clone(),
            },
            Decision::Pending { ask_id } => Self::Pending { ask_id: ask_id.0 },
        }
    }
}

impl From<Usage> for UsageDto {
    fn from(u: Usage) -> Self {
        Self {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cached_input_tokens: u.cached_input_tokens,
            // Anthropic populates `cache_creation_input_tokens`;
            // DashScope uses `cache_write_tokens`. Cost calc already
            // sums both server-side; surface them combined on the wire
            // so consumers see the same number that was billed.
            cache_write_tokens: u
                .cache_write_tokens
                .saturating_add(u.cache_creation_input_tokens),
            reasoning_tokens: u.reasoning_tokens,
        }
    }
}

impl From<AgentEvent> for EventDto {
    // COMPILE-TIME EXHAUSTIVENESS GUARD: this `match` has NO wildcard (`_`)
    // arm by design. Adding a variant to core `AgentEvent` therefore fails to
    // compile here until a matching `EventDto` arm is written — the DTO mirror
    // can never silently drop a new event kind. `event_dto_roundtrip.rs`
    // exercises every arm at runtime as the second backstop.
    //
    // The mirror is retained (rather than feature-gating `ToSchema` onto core
    // types) because the conversion is intentionally LOSSY on the wire —
    // `UsageDto` sums `cache_write_tokens + cache_creation_input_tokens` into
    // one field and drops `cost_usd`, and `PermissionAsked` folds `ask_id`
    // into `PermissionRequestDto`. A derived schema on the 7-field core `Usage`
    // would change the published OpenAPI contract the TUI consumes, so the
    // hand-written mirror is the contract boundary. See the plan's Phase 7
    // decision (Option (b)): keep the mirror + this exhaustiveness guard.
    fn from(ev: AgentEvent) -> Self {
        match ev {
            AgentEvent::SessionStatus {
                session_id,
                status,
                at,
            } => Self::SessionStatus {
                session_id: session_id.as_uuid(),
                status,
                at,
            },
            AgentEvent::MessageCreated {
                session_id,
                message_id,
                at,
            } => Self::MessageCreated {
                session_id: session_id.as_uuid(),
                message_id: message_id.as_uuid(),
                at,
            },
            AgentEvent::PartCreated {
                session_id,
                message_id,
                part_id,
                at,
            } => Self::PartCreated {
                session_id: session_id.as_uuid(),
                message_id: message_id.as_uuid(),
                part_id: part_id.as_uuid(),
                at,
            },
            AgentEvent::PartDelta {
                session_id,
                message_id,
                part_id,
                delta_kind,
                delta,
            } => Self::PartDelta {
                session_id: session_id.as_uuid(),
                message_id: message_id.as_uuid(),
                part_id: part_id.as_uuid(),
                delta_kind: delta_kind.into(),
                delta,
            },
            AgentEvent::PartUpdated {
                session_id,
                message_id,
                part_id,
            } => Self::PartUpdated {
                session_id: session_id.as_uuid(),
                message_id: message_id.as_uuid(),
                part_id: part_id.as_uuid(),
            },
            AgentEvent::StepFinished {
                session_id,
                message_id,
                reason,
                usage,
                cost_decimal_str,
            } => Self::StepFinished {
                session_id: session_id.as_uuid(),
                message_id: message_id.as_uuid(),
                reason,
                usage: usage.map(UsageDto::from),
                cost_decimal_str,
            },
            AgentEvent::PermissionAsked {
                session_id,
                ask_id,
                request,
            } => Self::PermissionAsked {
                session_id: session_id.as_uuid(),
                request: PermissionRequestDto::from_request(ask_id.0, &request),
            },
            AgentEvent::PermissionResolved {
                session_id,
                ask_id,
                decision,
            } => Self::PermissionResolved {
                session_id: session_id.as_uuid(),
                ask_id: ask_id.0,
                decision: PermissionDecisionDto::from_decision(&decision),
            },
            AgentEvent::DetachedToolAuthorized {
                session_id,
                tool,
                request,
                decision,
                ..
            } => Self::DetachedToolAuthorized {
                session_id: session_id.as_uuid(),
                tool,
                request: PermissionRequestDto::from_request(Uuid::nil(), &request),
                decision: PermissionDecisionDto::from_decision(&decision),
            },
            AgentEvent::Error {
                session_id,
                code,
                message,
            } => Self::Error {
                session_id: session_id.map(|s| s.as_uuid()),
                code,
                message,
            },
            AgentEvent::PluginError {
                session_id,
                plugin_id,
                hook,
                message,
            } => Self::PluginError {
                session_id: session_id.map(|s| s.as_uuid()),
                plugin_id,
                hook,
                message,
            },
            AgentEvent::QuestionRequested {
                session_id,
                question_id,
                header,
                question,
                options,
                multi_select,
            } => Self::QuestionRequested {
                session_id: session_id.as_uuid(),
                question_id: question_id.as_uuid(),
                header,
                question,
                options: options.into_iter().map(AskOptionDto::from).collect(),
                multi_select,
            },
            AgentEvent::PlanModeEntered { session_id, at } => Self::PlanModeEntered {
                session_id: session_id.as_uuid(),
                at,
            },
            AgentEvent::PlanModeExited {
                session_id,
                plan,
                at,
            } => Self::PlanModeExited {
                session_id: session_id.as_uuid(),
                plan,
                at,
            },
            AgentEvent::AttachmentAccepted {
                session_id,
                message_id,
                part_id,
                artifact_id,
                attachment_kind,
                mime,
                summary,
            } => Self::AttachmentAccepted {
                session_id: session_id.as_uuid(),
                message_id: message_id.as_uuid(),
                part_id: part_id.as_uuid(),
                artifact_id,
                attachment_kind: attachment_kind.into(),
                mime,
                summary,
            },
            AgentEvent::SubagentSpawned {
                task_id,
                tool_call_id,
                child_session_id,
                parent_session_id,
                subagent_type,
                objective,
                description,
                background,
            } => Self::SubagentSpawned {
                task_id,
                tool_call_id,
                child_session_id: child_session_id.as_uuid(),
                parent_session_id: parent_session_id.as_uuid(),
                subagent_type,
                objective,
                description,
                background,
            },
            AgentEvent::SubagentProgress {
                task_id,
                parent_session_id,
                delta,
            } => Self::SubagentProgress {
                task_id,
                parent_session_id: parent_session_id.as_uuid(),
                delta,
            },
            AgentEvent::SubagentSettled {
                task_id,
                child_session_id,
                parent_session_id,
                status,
                cost_usd,
            } => Self::SubagentSettled {
                task_id,
                child_session_id: child_session_id.as_uuid(),
                parent_session_id: parent_session_id.as_uuid(),
                status,
                cost_usd,
            },
            AgentEvent::SubagentMessage {
                task_id,
                parent_session_id,
                from,
                to,
            } => Self::SubagentMessage {
                task_id,
                parent_session_id: parent_session_id.as_uuid(),
                from,
                to,
            },
            AgentEvent::SubagentRoster {
                root_session_id,
                entries,
            } => Self::SubagentRoster {
                root_session_id: root_session_id.as_uuid(),
                entries: entries
                    .into_iter()
                    .map(|e| RosterEntryDto {
                        name: e.name,
                        task_id: e.task_id,
                        generation: e.generation,
                    })
                    .collect(),
            },
            AgentEvent::NotificationEmitted {
                session_id,
                level,
                title,
                body,
                plugin_id,
            } => Self::NotificationEmitted {
                session_id: session_id.map(|s| s.as_uuid()),
                level: level.into(),
                title,
                body,
                plugin_id,
            },
            AgentEvent::TodoUpdated { session_id, items } => Self::TodoUpdated {
                session_id: session_id.as_uuid(),
                items: items.into_iter().map(TodoItemDto::from).collect(),
            },
            AgentEvent::Heartbeat => Self::Heartbeat,
        }
    }
}
