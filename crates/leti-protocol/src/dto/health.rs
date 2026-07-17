//! Health response DTO.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Body of `GET /v1/health`.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthDto {
    /// `true` once the server has finished boot.
    pub ok: bool,
    /// `CARGO_PKG_VERSION` of the running binary.
    #[schema(example = "0.1.0")]
    pub version: String,
}
