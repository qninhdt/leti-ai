//! Message DTOs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use leti_core::types::message::{Message, Role};

use super::part::PartDto;

/// `POST /v1/session/:id/prompt_async` body.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateMessageDto {
    pub parts: Vec<PartDto>,
}

/// `POST /v1/session/:id/prompt_async` ack.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PromptAckDto {
    pub message_id: Uuid,
    pub ack: bool,
}

/// Public projection of a `Message`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MessageDto {
    pub id: Uuid,
    pub session_id: Uuid,
    pub role: Role,
    pub created_at: DateTime<Utc>,
    pub parts: Vec<PartDto>,
}

impl MessageDto {
    #[must_use]
    pub fn from_message(msg: Message, parts: Vec<PartDto>) -> Self {
        Self {
            id: msg.id.as_uuid(),
            session_id: msg.session_id.as_uuid(),
            role: msg.role,
            created_at: msg.created_at,
            parts,
        }
    }
}
