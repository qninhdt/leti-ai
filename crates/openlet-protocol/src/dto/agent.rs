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
    /// The model the serve path actually runs for this agent's turns.
    /// `AgentSpec` carries no per-agent model; turns resolve the
    /// effective model from `config.default_model`, so the route fills
    /// this with that value to avoid showing a model the turn never uses.
    pub model: String,
}

impl AgentDto {
    /// Build from a spec plus the effective serve model. Use this on the
    /// list route so the status bar shows the model turns actually run.
    #[must_use]
    pub fn from_spec_with_model(spec: &AgentSpec, model: impl Into<String>) -> Self {
        Self {
            id: spec.id.as_uuid(),
            display_name: spec.display_name.clone(),
            workspace_root: spec.workspace_root.display().to_string(),
            model: model.into(),
        }
    }

    #[must_use]
    pub fn agent_id(&self) -> AgentId {
        AgentId::from(self.id)
    }
}
