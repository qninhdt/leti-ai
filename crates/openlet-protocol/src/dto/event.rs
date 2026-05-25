//! Event DTOs for SSE frames.
//!
//! Wire shape mirrors `openlet_core::types::event::AgentEvent` 1:1; we
//! re-derive `ToSchema` here so utoipa picks the type up. Field
//! ordering matches the durable JSON we already write to the `events`
//! table — no double-translation cost.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use openlet_core::types::event::{AgentEvent, AskOption, AttachmentKind, DeltaKind, Usage};
use openlet_core::types::session::SessionStatus;

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
        decision: String,
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
    Heartbeat,
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
            AttachmentKind::Image => Self::Image,
            AttachmentKind::Document => Self::Document,
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
            DeltaKind::Text => Self::Text,
            DeltaKind::Reasoning => Self::Reasoning,
            DeltaKind::ToolArgs => Self::ToolArgs,
        }
    }
}

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

impl From<Usage> for UsageDto {
    fn from(u: Usage) -> Self {
        Self {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
            cached_input_tokens: u.cached_input_tokens,
            cache_write_tokens: u.cache_write_tokens,
            reasoning_tokens: u.reasoning_tokens,
        }
    }
}

impl From<AgentEvent> for EventDto {
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
                decision: decision_label(&decision),
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
            AgentEvent::Heartbeat => Self::Heartbeat,
        }
    }
}

/// Stable wire label for a `Decision`. Server-side enum carries a
/// `feedback` payload but the wire format only needs the outcome.
fn decision_label(d: &openlet_core::types::permission::Decision) -> String {
    use openlet_core::types::permission::Decision;
    match d {
        Decision::Allow => "allow".to_string(),
        Decision::Deny { .. } => "deny".to_string(),
        Decision::Pending { .. } => "pending".to_string(),
    }
}
