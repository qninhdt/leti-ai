//! `/v1/plugin*` — plugin discovery + health (plugin-system §6).
//!
//! Lightweight surface: list registered plugins by name, report health
//! placeholder until the manifest layer plumbs through manifests with
//! semver + capability metadata.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::types::event::AgentEvent;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::app_state::AppState;
use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PluginInfoDto {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PluginHealthDto {
    pub id: String,
    pub healthy: bool,
}

#[utoipa::path(
    get,
    path = "/v1/plugin",
    tag = "plugin",
    responses(
        (status = 200, description = "Registered plugins", body = [PluginInfoDto])
    )
)]
pub async fn list(State(state): State<AppState>) -> Json<Vec<PluginInfoDto>> {
    let plugins = state
        .plugin_registry
        .iter()
        .map(|p| PluginInfoDto {
            id: p.manifest().id.clone(),
            status: "registered".to_string(),
        })
        .collect();
    Json(plugins)
}

#[utoipa::path(
    get,
    path = "/v1/plugin/{id}/health",
    tag = "plugin",
    params(("id" = String, Path, description = "Plugin id")),
    responses(
        (status = 200, description = "Plugin health", body = PluginHealthDto),
        (status = 404, description = "Plugin not found"),
    )
)]
pub async fn health(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PluginHealthDto>, AppError> {
    let found = state.plugin_registry.iter().any(|p| p.manifest().id == id);
    if !found {
        let _ = state
            .events
            .publish(
                AgentEvent::Error {
                    session_id: None,
                    code: "plugin_not_found".to_string(),
                    message: format!("plugin {id} not registered"),
                },
                Persistence::Durable,
            )
            .await;
        return Err(AppError::not_found("plugin_not_found", "plugin not found"));
    }
    Ok(Json(PluginHealthDto { id, healthy: true }))
}

#[allow(dead_code)]
const _: StatusCode = StatusCode::OK;
