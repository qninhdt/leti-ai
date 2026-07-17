//! Error response body for all 4xx/5xx HTTP responses.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Stable error envelope. `code` is a `&'static str` slug from
/// `FailureClass::as_str` (or a route-local slug); `message` is a
/// human-readable detail. `details` carries optional structured context
/// (validation errors, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorDto {
    #[schema(example = "session_not_found")]
    pub code: String,
    #[schema(example = "session 7f… not found")]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
