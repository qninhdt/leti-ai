//! End-to-end test: `test-quota-stub` cancels sessions when budgets
//! deplete. Drives the plugin through real `install` + dispatch — the
//! same code path the server runs at boot. A recording `CoreApi` stub
//! captures `cancel_session` calls so each scenario can assert the
//! exact signal flow:
//!
//! 1. budget high enough → `on_cost_tick` continues, no cancel
//! 2. budget pre-exhausted → `before_turn` returns `Stop` (no model
//!    call charged)
//! 3. cost tick drives balance ≤ 0 → `cancel_session("budget_exhausted")`
//!    fires AND the hook returns `Stop`
//!
//! The active-turn cancellation token is NOT tested here — that
//! belongs to the server crate where `active_turns` lives. This test
//! pins the *plugin*'s contract: given the inputs phase 4 + 5 deliver,
//! does the plugin do the right thing?

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::session::{SessionId, SessionMeta, SessionStatus};
use openlet_plugin_api::context::{CoreApi, PluginContext};
use openlet_plugin_api::dispatch::{DispatchOutcome, dispatch};
use openlet_plugin_api::hooks::io::{BeforeTurnCtx, OnCostTickCtx};
use openlet_plugin_api::plugin::Plugin;
use openlet_plugin_test_quota_stub::QuotaStubPlugin;
use rust_decimal::Decimal;
use serde_json::json;

/// Minimal recording `CoreApi` — captures `cancel_session` calls and
/// returns a fixed `SessionMeta` shaped exactly like the integrator's
/// real auth blob (`extensions["user_id"] = "u1"`).
struct RecordingCore {
    meta: SessionMeta,
    cancellations: Mutex<Vec<(SessionId, String)>>,
}

impl RecordingCore {
    fn new(user_id: &str) -> Arc<Self> {
        let id = SessionId::new();
        let now = chrono::Utc::now();
        let meta = SessionMeta {
            id,
            agent_id: AgentId::new(),
            status: SessionStatus::Running,
            permission_mode: openlet_core::types::permission::PermissionMode::default(),
            parent_session_id: None,
            created_at: now,
            updated_at: now,
            deleted_at: None,
            version: "0.1.0".into(),
            extensions: json!({"user_id": user_id}),
            capabilities: openlet_core::types::session::SessionCapabilities::default(),
            current_agent_slug: None,
            previous_agent_slug: None,
            depth: 0,
        };
        Arc::new(Self {
            meta,
            cancellations: Mutex::new(Vec::new()),
        })
    }

    fn session_id(&self) -> SessionId {
        self.meta.id
    }

    fn cancellations(&self) -> Vec<(SessionId, String)> {
        self.cancellations.lock().unwrap().clone()
    }
}

#[async_trait]
impl CoreApi for RecordingCore {
    async fn current_session_meta(&self, session_id: SessionId) -> Option<SessionMeta> {
        if session_id == self.meta.id {
            Some(self.meta.clone())
        } else {
            None
        }
    }
    fn session_cost(&self, _: SessionId) -> Decimal {
        Decimal::ZERO
    }
    fn record_cost(&self, _: SessionId, _: Decimal) {}
    async fn emit_event(&self, _: AgentEvent, _: Persistence) {}
    fn read_config(&self, _: &str) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Null)
    }
    async fn cancel_session(&self, session_id: SessionId, reason: String) {
        self.cancellations
            .lock()
            .unwrap()
            .push((session_id, reason));
    }
}

/// Install the plugin and drain its chains. Mirrors what
/// `install_all` does at boot, minus the registry merge.
async fn install_with_budgets(
    core: Arc<dyn CoreApi>,
    budgets: serde_json::Value,
) -> openlet_plugin_api::context::PluginRegistrations {
    let plugin = QuotaStubPlugin::new();
    let manifest = plugin.manifest().clone();
    let mut ctx = PluginContext::new(manifest, json!({"budgets": budgets}), core);
    plugin
        .install(&mut ctx)
        .await
        .expect("install must succeed");
    ctx.into_registrations()
}

#[tokio::test]
async fn high_budget_completes_without_cancellation() {
    let core_handle = RecordingCore::new("u1");
    let core: Arc<dyn CoreApi> = core_handle.clone();
    let regs = install_with_budgets(core, json!({"u1": "100"})).await;

    // Cost tick of $0.01 — well under the $100 budget. on_cost_tick
    // must continue; before_turn must continue.
    let cost = OnCostTickCtx {
        session_id: Some(core_handle.session_id()),
        model: "stub-model".into(),
        delta_usd: Some(Decimal::new(1, 2)),
        total_usd: Decimal::new(1, 2),
        usage: None,
    };
    match dispatch(&regs.chains.on_cost_tick, cost).await {
        DispatchOutcome::Completed(_) => {}
        other => panic!("expected Completed, got {other:?}"),
    }
    let before = BeforeTurnCtx {
        session_id: Some(core_handle.session_id()),
        turn_index: 1,
        message_count: 1,
    };
    match dispatch(&regs.chains.before_turn, before).await {
        DispatchOutcome::Completed(_) => {}
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(
        core_handle.cancellations().is_empty(),
        "high budget must not trigger cancel"
    );
}

#[tokio::test]
async fn before_turn_stops_when_budget_pre_exhausted() {
    let core_handle = RecordingCore::new("u1");
    let core: Arc<dyn CoreApi> = core_handle.clone();
    // Zero balance from the start — before_turn must short-circuit
    // before any model call is issued (no cost charged).
    let regs = install_with_budgets(core, json!({"u1": "0"})).await;

    let before = BeforeTurnCtx {
        session_id: Some(core_handle.session_id()),
        turn_index: 0,
        message_count: 0,
    };
    match dispatch(&regs.chains.before_turn, before).await {
        DispatchOutcome::Stopped(_) => {}
        other => panic!("expected Stopped (budget exhausted), got {other:?}"),
    }
    // Pre-exhaustion path doesn't go through on_cost_tick, so
    // cancel_session is NOT called here — the loop terminates with
    // FinishReason::Halted on the runtime side instead.
    assert!(
        core_handle.cancellations().is_empty(),
        "before_turn stop must not call cancel_session — that's the runtime's job",
    );
}

#[tokio::test]
async fn cost_tick_cancels_session_when_balance_drains() {
    let core_handle = RecordingCore::new("u1");
    let core: Arc<dyn CoreApi> = core_handle.clone();
    // $0.01 starting balance — a $0.05 turn drains it.
    let regs = install_with_budgets(core, json!({"u1": "0.01"})).await;

    let cost = OnCostTickCtx {
        session_id: Some(core_handle.session_id()),
        model: "stub-model".into(),
        delta_usd: Some(Decimal::new(5, 2)),
        total_usd: Decimal::new(5, 2),
        usage: None,
    };
    match dispatch(&regs.chains.on_cost_tick, cost).await {
        DispatchOutcome::Stopped(_) => {}
        other => panic!("expected Stopped (budget drained), got {other:?}"),
    }
    let cancels = core_handle.cancellations();
    assert_eq!(cancels.len(), 1, "exactly one cancel_session call");
    assert_eq!(cancels[0].0, core_handle.session_id());
    assert_eq!(cancels[0].1, "budget_exhausted");

    // After the drain, before_turn must also Stop — defends against
    // the runtime racing to start the next turn before cancellation
    // propagates.
    let before = BeforeTurnCtx {
        session_id: Some(core_handle.session_id()),
        turn_index: 1,
        message_count: 1,
    };
    match dispatch(&regs.chains.before_turn, before).await {
        DispatchOutcome::Stopped(_) => {}
        other => panic!("expected Stopped after drain, got {other:?}"),
    }
}

#[tokio::test]
async fn missing_user_id_skips_silently() {
    // Sessions without `extensions["user_id"]` (local dev, no
    // integrator auth blob) must NOT panic and must NOT trigger
    // cancellation. The plugin treats them as unmetered.
    let id = SessionId::new();
    let now = chrono::Utc::now();
    let core_handle = Arc::new(RecordingCore {
        meta: SessionMeta {
            id,
            agent_id: AgentId::new(),
            status: SessionStatus::Running,
            permission_mode: openlet_core::types::permission::PermissionMode::default(),
            parent_session_id: None,
            created_at: now,
            updated_at: now,
            deleted_at: None,
            version: "0.1.0".into(),
            extensions: serde_json::Value::Null,
            capabilities: openlet_core::types::session::SessionCapabilities::default(),
            current_agent_slug: None,
            previous_agent_slug: None,
            depth: 0,
        },
        cancellations: Mutex::new(Vec::new()),
    });
    let core: Arc<dyn CoreApi> = core_handle.clone();
    let regs = install_with_budgets(core, json!({"u1": "0"})).await;

    let cost = OnCostTickCtx {
        session_id: Some(id),
        model: "stub-model".into(),
        delta_usd: Some(Decimal::new(1, 0)),
        total_usd: Decimal::new(1, 0),
        usage: None,
    };
    match dispatch(&regs.chains.on_cost_tick, cost).await {
        DispatchOutcome::Completed(_) => {}
        other => panic!("expected Completed (no user_id), got {other:?}"),
    }
    assert!(core_handle.cancellations().is_empty());
}
