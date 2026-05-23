//! Part DTOs — message-content building blocks.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use openlet_core::types::part::{Part, PartId};

/// Wire shape for `Part`. Mirrors the domain enum 1:1 but skips the
/// `Image.bytes` field (images surface via the artifact store URL,
/// not inline JSON).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PartDto {
    Text {
        id: Uuid,
        text: String,
    },
    Reasoning {
        id: Uuid,
        text: String,
    },
    ToolCall {
        id: Uuid,
        call_id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        id: Uuid,
        call_id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Image {
        id: Uuid,
        mime: String,
    },
    StepStart {
        id: Uuid,
    },
    StepFinish {
        id: Uuid,
        reason: String,
    },
    Compaction {
        id: Uuid,
        summary: String,
        compacted_message_ids: Vec<String>,
        original_token_count: u32,
    },
}

impl From<Part> for PartDto {
    fn from(p: Part) -> Self {
        match p {
            Part::Text { id, text } => Self::Text { id: id.as_uuid(), text },
            Part::Reasoning { id, text } => Self::Reasoning { id: id.as_uuid(), text },
            Part::ToolCall { id, call_id, name, args } => {
                Self::ToolCall { id: id.as_uuid(), call_id, name, args }
            }
            Part::ToolResult { id, call_id, ok, text, error } => Self::ToolResult {
                id: id.as_uuid(),
                call_id,
                ok,
                text,
                error,
            },
            Part::Image { id, mime, .. } => Self::Image { id: id.as_uuid(), mime },
            Part::StepStart { id } => Self::StepStart { id: id.as_uuid() },
            Part::StepFinish { id, reason } => Self::StepFinish { id: id.as_uuid(), reason },
            Part::Compaction {
                id,
                summary,
                compacted_message_ids,
                original_token_count,
            } => Self::Compaction {
                id: id.as_uuid(),
                summary,
                compacted_message_ids,
                original_token_count,
            },
        }
    }
}

impl PartDto {
    /// Best-effort conversion back to a domain `Part`. Image bytes are
    /// not round-tripped (server side only).
    #[must_use]
    pub fn into_part_for_user_input(self) -> Option<Part> {
        match self {
            Self::Text { id, text } => Some(Part::Text { id: PartId(id), text }),
            Self::Reasoning { id, text } => Some(Part::Reasoning { id: PartId(id), text }),
            // The remaining variants are produced by the runtime, not
            // by user input. `prompt_async` only accepts Text/Reasoning.
            _ => None,
        }
    }
}
