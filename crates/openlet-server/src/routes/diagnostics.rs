//! `GET /v1/diagnostics` — REST face of the `/doctor` preflight.
//!
//! Same redacted [`DoctorReport`] the CLI emits, served as JSON so the
//! TUI `/doctor` slash command can render it inline. Read-only; no
//! writes, no auth-mutating side effects.

use axum::Json;
use axum::extract::State;
use serde_json::Value;

use crate::app_state::AppState;
use crate::diagnostics::{DoctorReport, run_diagnostics};

/// `GET /v1/diagnostics`
#[utoipa::path(
    get,
    path = "/v1/diagnostics",
    tag = "diagnostics",
    responses(
        (status = 200, description = "Preflight report", body = DoctorReport)
    )
)]
pub async fn report(State(state): State<AppState>) -> Json<Value> {
    let report = run_diagnostics(&state).await;
    Json(report.to_redacted_json())
}
