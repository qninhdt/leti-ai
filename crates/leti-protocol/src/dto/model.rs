//! Model-catalog DTO for `GET /v1/models`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use leti_core::adapters::model_provider::ModelInfo;

/// One model entry in the `GET /v1/models` response. Thin HTTP face over
/// [`ModelInfo`]; `id` is always present, the rest are best-effort
/// enrichment the upstream catalog may omit.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelDto {
    /// Canonical model id used in a turn's `model` field.
    pub id: String,
    /// Human-readable label when the catalog provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Maximum context window in tokens, when advertised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u32>,
}

impl From<ModelInfo> for ModelDto {
    fn from(m: ModelInfo) -> Self {
        Self {
            id: m.id,
            display_name: m.display_name,
            context_length: m.context_length,
        }
    }
}
