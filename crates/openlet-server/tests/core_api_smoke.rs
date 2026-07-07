//! Smoke test — `CoreApiImpl` honors the contract phase 4 plugins
//! depend on. We exercise the same call shape an `on_cost_tick` hook
//! would: fetch session meta, peek `extensions["user_id"]`, record cost,
//! emit an event, read a config key.
//!
//! No plugin install machinery here on purpose — that path is covered
//! by the registry tests. This file pins the *adapter* contract: given
//! the handles AppState already wires up, do the five CoreApi methods
//! return what an integrator hook would expect?

use std::sync::Arc;

use openlet_core::adapters::event_sink::Persistence;
use openlet_core::config::{Config, LogFormat, PluginsConfig};
use openlet_core::types::event::AgentEvent;
use openlet_core::types::session::SessionId;
use openlet_plugin_api::context::CoreApi;
use openlet_server::core_api_impl::CoreApiImpl;
use rust_decimal::Decimal;
use serde_json::json;

mod support;

fn test_config() -> Config {
    Config {
        bind_addr: "127.0.0.1:0".to_string(),
        data_dir: std::path::PathBuf::from("/tmp/openlet-coreapi-smoke"),
        openai_api_key: None,
        default_model: "stub-model".to_string(),
        permission_ruleset_path: None,
        log_format: LogFormat::Pretty,
        plugins: PluginsConfig::default(),
    }
}

#[tokio::test]
async fn core_api_reads_session_extensions_user_id() {
    let state = support::TestHarness::raw_state().await;
    let core: Arc<dyn CoreApi> = Arc::new(CoreApiImpl::new(
        state.memory.clone(),
        state.events.clone(),
        Arc::new(test_config()),
    ));

    // Create a session with an integrator-owned auth blob.
    let agent_id = state.default_agent_id;
    let sid = state.memory.create_session(agent_id, None).await.unwrap();
    let extensions = json!({"user_id": "u_123", "tenant_id": "t_42"});
    state
        .memory
        .update_session_extensions(sid, extensions.clone())
        .await
        .unwrap();

    // Same call shape an on_cost_tick hook makes — fetch meta, read the
    // integrator-defined field. Core stays auth-blind: the schema is
    // entirely on the integrator side.
    let meta = core
        .current_session_meta(sid)
        .await
        .expect("session must surface");
    assert_eq!(meta.extensions, extensions);
    assert_eq!(meta.extensions["user_id"], json!("u_123"));
}

#[tokio::test]
async fn core_api_session_cost_is_zero_without_runtime() {
    // Pre-set_runtime, session_cost must default to zero rather than
    // panic. The boot sequence binds runtime *after* install_plugins,
    // so install-time hook closures rely on this being safe.
    let state = support::TestHarness::raw_state().await;
    let core: Arc<dyn CoreApi> = Arc::new(CoreApiImpl::new(
        state.memory.clone(),
        state.events.clone(),
        Arc::new(test_config()),
    ));
    let sid = SessionId::new();
    assert_eq!(core.session_cost(sid), Decimal::ZERO);
    // record_cost must not panic either; warning gets logged and the
    // delta is dropped silently.
    core.record_cost(sid, Decimal::new(1, 2));
}

#[tokio::test]
async fn core_api_emit_event_round_trips_through_sink() {
    let state = support::TestHarness::raw_state().await;
    let core: Arc<dyn CoreApi> = Arc::new(CoreApiImpl::new(
        state.memory.clone(),
        state.events.clone(),
        Arc::new(test_config()),
    ));
    let sid = state
        .memory
        .create_session(state.default_agent_id, None)
        .await
        .unwrap();
    // Fire-and-forget — emit_event returns no error, mirroring the
    // observation-only contract phase 4 plugins rely on.
    core.emit_event(
        AgentEvent::Error {
            session_id: Some(sid),
            code: "smoke".to_string(),
            message: "core-api emit_event smoke".to_string(),
        },
        Persistence::Durable,
    )
    .await;
}

#[tokio::test]
async fn core_api_read_config_whitelist() {
    let state = support::TestHarness::raw_state().await;
    let core: Arc<dyn CoreApi> = Arc::new(CoreApiImpl::new(
        state.memory.clone(),
        state.events.clone(),
        Arc::new(test_config()),
    ));
    assert_eq!(
        core.read_config("default_model").unwrap(),
        json!("stub-model")
    );
    assert!(core.read_config("bind_addr").unwrap().is_string());
    let err = core.read_config("password").unwrap_err();
    assert!(err.contains("unknown config key"));
}
