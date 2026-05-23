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

use openlet_core::types::event::{AgentEvent, DeltaKind, Usage};
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
        ask_id: Uuid,
        decision: String,
    },
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<Uuid>,
        code: String,
        message: String,
    },
    Heartbeat,
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
            AgentEvent::SessionStatus { session_id, status, at } => Self::SessionStatus {
                session_id: session_id.as_uuid(),
                status,
                at,
            },
            AgentEvent::MessageCreated { session_id, message_id, at } => Self::MessageCreated {
                session_id: session_id.as_uuid(),
                message_id: message_id.as_uuid(),
                at,
            },
            AgentEvent::PartCreated { session_id, message_id, part_id, at } => Self::PartCreated {
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
            AgentEvent::PartUpdated { session_id, message_id, part_id } => Self::PartUpdated {
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
            AgentEvent::PermissionAsked { session_id, ask_id, request } => Self::PermissionAsked {
                session_id: session_id.as_uuid(),
                request: PermissionRequestDto::from_request(ask_id.0, &request),
            },
            AgentEvent::PermissionResolved { ask_id, decision } => Self::PermissionResolved {
                ask_id: ask_id.0,
                decision: decision_label(&decision),
            },
            AgentEvent::Error { session_id, code, message } => Self::Error {
                session_id: session_id.map(|s| s.as_uuid()),
                code,
                message,
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
