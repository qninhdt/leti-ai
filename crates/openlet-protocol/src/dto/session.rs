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
///
/// `user_questions` declares whether the caller can answer interactive
/// `ask_user` prompts. Defaults to `true` because this DTO is used by
/// interactive frontends (the TUI); headless callers set it `false` so
/// `ask_user` fails fast instead of blocking on a UI that never replies.
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
    #[serde(default = "default_user_questions")]
    pub user_questions: bool,
}

/// `ask_user` capability defaults ON for this DTO — it is the interactive
/// create path. Headless integrators pass `false` explicitly.
fn default_user_questions() -> bool {
    true
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
    /// Slug of the agent profile the session is currently running.
    /// `null` ⇒ the runtime falls back to the default profile
    /// (typically `general`). Plan mode flips this to `plan`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_agent_slug: Option<String>,
    /// Slug of the profile the session was on before the current
    /// `current_agent_slug` — used by `ExitPlanMode` to restore the
    /// prior profile. `null` for sessions that never switched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_agent_slug: Option<String>,
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
            current_agent_slug: m.current_agent_slug,
            previous_agent_slug: m.previous_agent_slug,
        }
    }
}

/// `POST /v1/session/:id/abort` ack.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AbortAckDto {
    pub aborted: bool,
}

/// Result of moving a running foreground subagent to the durable background
/// delivery path. The task and child session are unchanged.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct BackgroundTaskAckDto {
    pub task_id: uuid::Uuid,
    pub status: String,
}

/// `POST /v1/session/:id/compact` ack. `compacted` is false when there was
/// nothing to compact (conversation at/under the preserved-recent floor).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CompactAckDto {
    pub compacted: bool,
}
