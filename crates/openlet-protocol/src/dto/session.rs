//! Session DTOs.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use openlet_core::types::permission::PermissionMode;
use openlet_core::types::session::{SessionMeta, SessionStatus};

/// `POST /v1/session` body. `agent_id` may be omitted to use the
/// server's default agent (single-agent self-hosted boot).
///
/// `extensions` is an opaque integrator-owned JSON blob (e.g.
/// `{"user_id": "u_123", "tenant_id": "t_42"}`). Core stays auth-blind:
/// the schema lives entirely in the integrator. Defaults to `null`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateSessionDto {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    #[schema(value_type = Object)]
    pub extensions: serde_json::Value,
}

/// `POST /v1/session/:id/mode` body.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SetModeDto {
    pub mode: PermissionMode,
}

/// Public projection of `SessionMeta`.
///
/// `extensions` echoes back the integrator-owned blob set at create
/// time (or mutated since via `update_session_extensions`). Defaults
/// to `null` when the session has no extensions.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SessionDto {
    pub id: Uuid,
    pub agent_id: Uuid,
    pub status: SessionStatus,
    pub permission_mode: PermissionMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<DateTime<Utc>>,
    pub version: String,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    #[schema(value_type = Object)]
    pub extensions: serde_json::Value,
}

impl From<SessionMeta> for SessionDto {
    fn from(m: SessionMeta) -> Self {
        Self {
            id: m.id.as_uuid(),
            agent_id: m.agent_id.as_uuid(),
            status: m.status,
            permission_mode: m.permission_mode,
            parent_session_id: m.parent_session_id.map(|s| s.as_uuid()),
            created_at: m.created_at,
            updated_at: m.updated_at,
            deleted_at: m.deleted_at,
            version: m.version,
            extensions: m.extensions,
        }
    }
}

/// `POST /v1/session/:id/abort` ack.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AbortAckDto {
    pub aborted: bool,
}
