//! `/v1/agent` — list registered agents.

use axum::Json;
use axum::extract::State;
use openlet_protocol::AgentDto;

use crate::app_state::AppState;

/// `GET /v1/agent`
#[utoipa::path(
    get,
    path = "/v1/agent",
    tag = "agent",
    responses(
        (status = 200, description = "List of registered agents", body = [AgentDto])
    )
)]
pub async fn list(State(state): State<AppState>) -> Json<Vec<AgentDto>> {
    let agents: Vec<AgentDto> = state
        .agents
        .values()
        .map(|res| AgentDto::from(&res.spec))
        .collect();
    Json(agents)
}
