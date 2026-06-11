//! Phase-07 agent-registration smoke test: installing the `core-agents`
//! plugin populates the registry with `general` + `indexer`.

use openlet_core::adapters::event_sink::Persistence;
use openlet_core::agent::{AgentRegistry, AgentSlug};
use openlet_core::types::event::AgentEvent;
use openlet_core::types::session::{SessionId, SessionMeta};
use openlet_plugin_api::Plugin;
use openlet_plugin_api::PluginContext;
use openlet_plugin_api::context::CoreApi;
use openlet_plugin_core_agents::CoreAgentsPlugin;

struct StubCore;

#[async_trait::async_trait]
impl CoreApi for StubCore {
    async fn current_session_meta(&self, _: SessionId) -> Option<SessionMeta> {
        None
    }
    fn session_cost(&self, _: SessionId) -> rust_decimal::Decimal {
        rust_decimal::Decimal::ZERO
    }
    fn record_cost(&self, _: SessionId, _: rust_decimal::Decimal) {}
    async fn emit_event(&self, _: AgentEvent, _: Persistence) {}
    fn read_config(&self, _: &str) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Null)
    }
    async fn cancel_session(&self, _: SessionId, _: String) {}
    async fn emit_notification(
        &self,
        _: Option<SessionId>,
        _: openlet_core::hooks::io::NotificationLevel,
        _: String,
        _: String,
        _: String,
    ) {
    }
}

#[tokio::test]
async fn core_agents_plugin_registers_general_and_indexer() {
    let plugin = CoreAgentsPlugin::new();
    let manifest = plugin.manifest().clone();
    let mut ctx = PluginContext::new(
        manifest,
        serde_json::Value::Null,
        std::sync::Arc::new(StubCore),
    );
    plugin.install(&mut ctx).await.expect("install");
    let agents = ctx.take_registered_agents();
    assert_eq!(agents.len(), 3);

    let mut registry = AgentRegistry::new();
    for def in agents {
        registry.insert(def).expect("insert");
    }
    let general = AgentSlug::new("general").unwrap();
    let indexer = AgentSlug::new("indexer").unwrap();
    let plan = AgentSlug::new("plan").unwrap();
    let g = registry.get(&general).expect("general present");
    let i = registry.get(&indexer).expect("indexer present");
    let p = registry.get(&plan).expect("plan present");

    // Semantic allowlist assertions — what each agent's role REQUIRES,
    // not a count that mirrors the source constant (a len() check passes
    // even if the wrong tools are swapped in/out).
    let has = |def: &openlet_core::agent::AgentDefinition, t: &str| {
        def.tool_allowlist.iter().any(|x| x == t)
    };

    // general: full write-capable catalog.
    assert!(has(g, "read") && has(g, "write") && has(g, "edit") && has(g, "bash"));

    // indexer: read-only — must have read tools, must NOT mutate.
    assert!(has(i, "read") && has(i, "list"));
    assert!(!has(i, "write") && !has(i, "edit") && !has(i, "bash"));

    // plan: read + plan-mode toggles, but never write/edit/bash (it
    // proposes a plan, it doesn't execute changes).
    assert!(has(p, "read") && has(p, "enter_plan_mode") && has(p, "exit_plan_mode"));
    assert!(!has(p, "write") && !has(p, "edit") && !has(p, "bash"));
    assert!(!g.cacheable_prompt().is_empty());
    assert!(!p.cacheable_prompt().is_empty());
}
