//! `GET /v1/health` — unauthenticated readiness probe.

use axum::Json;
use openlet_protocol::HealthDto;

/// Health-check handler. Always returns 200 once the server is bound.
#[utoipa::path(
    get,
    path = "/v1/health",
    tag = "global",
    responses(
        (status = 200, description = "Server is up", body = HealthDto)
    )
)]
pub async fn handler() -> Json<HealthDto> {
    Json(HealthDto {
        ok: true,
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}
