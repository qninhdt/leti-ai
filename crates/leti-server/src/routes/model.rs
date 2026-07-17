//! `/v1/models` — list the model catalog the active provider advertises.

use axum::Json;
use axum::extract::State;
use leti_protocol::ModelDto;

use crate::app_state::AppState;
use crate::error::AppError;

/// `GET /v1/models`
///
/// Delegates to [`ModelProvider::list_models`]. Providers without a
/// catalog (mock, single-model gateways) return `[]` via the trait's
/// default impl, so this route is always 200 on a reachable provider and
/// only surfaces an error when the upstream call itself fails (network,
/// auth, rate-limit).
#[utoipa::path(
    get,
    path = "/v1/models",
    tag = "global",
    responses(
        (status = 200, description = "Provider model catalog", body = [ModelDto]),
        (status = 502, description = "Upstream provider error", body = leti_protocol::ErrorDto)
    )
)]
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<ModelDto>>, AppError> {
    let models = state.provider.list_models().await?;
    Ok(Json(models.into_iter().map(ModelDto::from).collect()))
}
