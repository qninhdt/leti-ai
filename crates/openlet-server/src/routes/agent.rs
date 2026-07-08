//! `/v1/agent` — list registered agents.

use axum::Json;
use axum::extract::State;
use openlet_core::agent::AgentSlug;
use openlet_protocol::AgentDto;

use crate::app_state::AppState;

/// Fallback context tuning if the agent registry has no `general` definition
/// (shouldn't happen in a booted server). Mirrors the built-in constants in
/// `core-agents` so the client still gets a sane denominator.
const DEFAULT_CONTEXT_WINDOW: u32 = 200_000;
const DEFAULT_COMPACTION_THRESHOLD: f32 = 0.8;

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
    let model = state.config.default_model.as_str();
    // Context tuning lives on the agent definition, not the spec. Built-in
    // agents share one window/threshold; resolve from the canonical `general`
    // definition (the same slug the turn driver falls back to) and default to
    // the shared constants if the registry is somehow empty.
    let (context_window, compaction_threshold) = state
        .agent_registry
        .get(&AgentSlug::new("general").expect("static slug"))
        .map(|def| (def.context_window, def.compaction_threshold))
        .unwrap_or((DEFAULT_CONTEXT_WINDOW, DEFAULT_COMPACTION_THRESHOLD));
    let agents: Vec<AgentDto> = state
        .agents
        .values()
        .map(|res| {
            AgentDto::from_spec_with_model(&res.spec, model, context_window, compaction_threshold)
        })
        .collect();
    Json(agents)
}
