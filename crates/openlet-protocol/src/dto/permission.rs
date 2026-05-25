//! Permission DTOs.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use openlet_core::types::permission::{Decision, PermissionRequest};

/// `POST /v1/permission/:ask_id` body.
///
/// `decision` mirrors the runtime enum but adds two persisted variants:
/// `always_allow` and `always_deny` (recorded into `permission_decisions`
/// per amendment §E so future calls bypass the ask).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionReplyKind {
    Allow,
    Deny,
    AlwaysAllow,
    AlwaysDeny,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PermissionReplyDto {
    pub decision: PermissionReplyKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Permission pattern to persist when `decision` is `always_*`.
    /// Echoed verbatim from the original `permission.asked` event;
    /// ignored for one-shot `allow`/`deny`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

impl PermissionReplyDto {
    /// One-shot conversion to a runtime `Decision`. The `always_*`
    /// variants collapse to `Allow`/`Deny` for the in-flight ask; the
    /// route handler separately persists the rule before replying.
    #[must_use]
    pub fn to_decision(&self) -> Decision {
        match self.decision {
            PermissionReplyKind::Allow | PermissionReplyKind::AlwaysAllow => Decision::Allow,
            PermissionReplyKind::Deny | PermissionReplyKind::AlwaysDeny => Decision::Deny {
                feedback: self.reason.clone(),
            },
        }
    }

    #[must_use]
    pub fn is_persistent(&self) -> bool {
        matches!(
            self.decision,
            PermissionReplyKind::AlwaysAllow | PermissionReplyKind::AlwaysDeny
        )
    }
}

/// Public shape of a pending permission request emitted in
/// `permission.asked` events.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PermissionRequestDto {
    pub ask_id: Uuid,
    pub permission: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl PermissionRequestDto {
    #[must_use]
    pub fn from_request(ask_id: Uuid, req: &PermissionRequest) -> Self {
        Self {
            ask_id,
            permission: req.permission.clone(),
            reason: req.reason.clone(),
            timeout_ms: req.timeout.map(|d| d.as_millis() as u64),
        }
    }
}
