//! Live E2E — plugin + agent surface over a real loopback server.
//!
//! The plugin host internals (hook draining, quota cancel, agent
//! registration through `install_all`) are already covered by
//! `integration_smoke.rs`, `end_to_end_plugin_determinism.rs`, and
//! `quota_stop_on_credit_zero.rs`. This file adds the layer those skip:
//! the discovery surface served over real HTTP, the way the TUI's
//! plugins view + agent picker consume it.

use openlet_test_mock_provider::MockOpenAiService;
use serde_json::Value;

mod live_support;
use live_support::LiveServer;

/// `GET /v1/plugin` lists the canonical plugin set over real HTTP. The
/// harness installs the same plugins the binary does (`all_plugins`), so
/// the registered set must include the core contributors.
#[tokio::test]
async fn plugin_list_served_over_http() {
    let mock = MockOpenAiService::spawn().await.expect("mock");
    let srv = LiveServer::with_mock(mock.base_url()).await;

    let plugins = srv.get_json("/v1/plugin").await;
    let arr = plugins.as_array().expect("plugin list is an array");
    let ids: Vec<&str> = arr.iter().filter_map(|p| p["id"].as_str()).collect();

    // core-tools + core-agents register through the public plugin surface;
    // both must appear in the discovery endpoint the TUI reads.
    assert!(
        ids.iter().any(|id| id.contains("core-tools")),
        "expected core-tools in plugin list; got {ids:?}"
    );
    assert!(
        ids.iter().any(|id| id.contains("core-agents")),
        "expected core-agents in plugin list; got {ids:?}"
    );
}

/// `GET /v1/plugin/:id/health` returns healthy for a registered plugin
/// and 404 for an unknown one — the exact contract the TUI plugins view
/// relies on to render per-plugin status.
#[tokio::test]
async fn plugin_health_found_and_not_found() {
    let mock = MockOpenAiService::spawn().await.expect("mock");
    let srv = LiveServer::with_mock(mock.base_url()).await;

    // Discover a real id first, then probe its health.
    let plugins = srv.get_json("/v1/plugin").await;
    let id = plugins
        .as_array()
        .and_then(|a| a.first())
        .and_then(|p| p["id"].as_str());
    let id = id.expect("at least one plugin registered").to_string();

    let (status, body) = srv
        .get_with_status(&format!("/v1/plugin/{id}/health"))
        .await;
    assert_eq!(status, reqwest::StatusCode::OK, "health of {id}");
    assert_eq!(
        body["healthy"],
        Value::Bool(true),
        "plugin {id} should be healthy"
    );

    // Unknown plugin → 404 with the documented slug.
    let (status, body) = srv
        .get_with_status("/v1/plugin/does-not-exist/health")
        .await;
    assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
    assert_eq!(
        body["code"].as_str(),
        Some("plugin_not_found"),
        "expected plugin_not_found slug; got {body:?}"
    );
}

/// `GET /v1/agent` lists the registered agent(s) over real HTTP — the
/// source the TUI agent picker renders from.
#[tokio::test]
async fn agent_list_served_over_http() {
    let mock = MockOpenAiService::spawn().await.expect("mock");
    let srv = LiveServer::with_mock(mock.base_url()).await;

    let agents = srv.get_json("/v1/agent").await;
    let arr = agents.as_array().expect("agent list is an array");
    assert!(!arr.is_empty(), "expected at least one registered agent");
    // Each agent row carries the fields the picker needs.
    for a in arr {
        assert!(a["id"].is_string(), "agent row missing id: {a:?}");
        assert!(
            a.get("display_name").is_some() || a.get("name").is_some(),
            "agent row missing a display field: {a:?}"
        );
    }
}
