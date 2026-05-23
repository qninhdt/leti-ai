//! Agent DTO for `GET /v1/agent`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use openlet_core::types::agent::{AgentId, AgentSpec};

/// Public-facing description of a registered agent.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AgentDto {
    pub id: Uuid,
    pub display_name: String,
    pub workspace_root: String,
}

impl From<&AgentSpec> for AgentDto {
    fn from(spec: &AgentSpec) -> Self {
        Self {
            id: spec.id.as_uuid(),
            display_name: spec.display_name.clone(),
            workspace_root: spec.workspace_root.display().to_string(),
        }
    }
}

impl AgentDto {
    #[must_use]
    pub fn agent_id(&self) -> AgentId {
        AgentId::from(self.id)
    }
}
