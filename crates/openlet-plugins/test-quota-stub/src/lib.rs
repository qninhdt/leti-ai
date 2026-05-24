//! `test-quota-stub` — reference plugin demonstrating the cost-tick
//! cancel pattern that downstream integrators (Openlet Cloud) port.
//!
//! Wires two hooks against an in-memory `HashMap<UserId, Decimal>`
//! budget map (config-driven via `ctx.config()`):
//!
//! - `on_cost_tick`: reads `extensions["user_id"]`, decrements the
//!   per-user balance by `delta_usd`, and calls
//!   `CoreApi::cancel_session` if balance ≤ 0. The active turn unwinds.
//! - `before_turn`: re-reads the balance and returns
//!   `HookResult::Stop` if already exhausted, so the next turn never
//!   starts (avoids charging for a turn that gets cancelled mid-flight).
//!
//! Core stays auth-blind: the schema (`extensions["user_id"]`) is the
//! integrator's, not core's. Plugin authors swap the in-memory map for
//! a real billing service in their own port.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openlet_plugin_api::context::CoreApi;
use openlet_plugin_api::hooks::{
    HookKind, HookResult, Priority,
    io::{BeforeTurnCtx, OnCostTickCtx},
};
use openlet_plugin_api::manifest::Capability;
use openlet_plugin_api::{Plugin, PluginContext, PluginError, PluginManifest};
use rust_decimal::Decimal;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};

/// Per-plugin config block. Maps `user_id` → starting balance USD.
/// `default_priority` lets integrators tune ordering vs. other hooks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QuotaConfig {
    #[serde(default)]
    pub budgets: HashMap<String, Decimal>,
}

/// Shared, mutable balance map backing both hooks. Plain `Mutex` is
/// fine here — quota updates are a single-digit-microsecond op and the
/// hook timeout is 5 seconds, so the lock can never be a bottleneck.
type Balances = Arc<Mutex<HashMap<String, Decimal>>>;

pub struct QuotaStubPlugin {
    manifest: PluginManifest,
}

impl Default for QuotaStubPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl QuotaStubPlugin {
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "test-quota-stub".into(),
                name: "Test Quota Stub".into(),
                version: Version::new(0, 1, 0),
                description: "Reference quota plugin — stops the loop when a per-user budget is \
                              exhausted. Integrators fork this into their billing service."
                    .into(),
                author: Some("Openlet".into()),
                capabilities: vec![
                    Capability::Hook(HookKind::OnCostTick),
                    Capability::Hook(HookKind::BeforeTurn),
                ],
                core_version_req: VersionReq::parse(">=0.1.0").expect("static version req"),
                default_priority: 50,
                config_schema: None,
            },
        }
    }
}

/// Looks up `extensions["user_id"]` on the live session via `CoreApi`.
/// Returns `None` for unknown sessions, missing extensions, or non-string
/// `user_id`. Hooks that hit `None` skip silently — the integrator owns
/// what to do for sessions without a `user_id` shape.
async fn user_id_for_session(
    core: &Arc<dyn CoreApi>,
    session_id: openlet_core::types::session::SessionId,
) -> Option<String> {
    let meta = core.current_session_meta(session_id).await?;
    meta.extensions
        .get("user_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

#[async_trait]
impl Plugin for QuotaStubPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn install(&self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        let cfg: QuotaConfig = ctx.config().unwrap_or_default();
        let balances: Balances = Arc::new(Mutex::new(cfg.budgets));

        // on_cost_tick: charge the per-user balance and cancel mid-flight
        // if the just-finished turn drove balance ≤ 0.
        let core_for_cost = ctx.core();
        let balances_for_cost = balances.clone();
        ctx.on_cost_tick(Priority::default(), move |cost: OnCostTickCtx| {
            let core = core_for_cost.clone();
            let balances = balances_for_cost.clone();
            async move {
                let Some(session_id) = cost.session_id else {
                    return HookResult::Continue(cost);
                };
                let Some(user_id) = user_id_for_session(&core, session_id).await else {
                    return HookResult::Continue(cost);
                };

                let exhausted = {
                    let mut map = balances.lock().expect("balances mutex poisoned");
                    // Skip silently for users not in the budget map —
                    // unmetered. An implicit-zero default would cancel
                    // every unknown user on the first cost tick, which
                    // is a footgun for integrators porting this plugin.
                    let Some(entry) = map.get_mut(&user_id) else {
                        return HookResult::Continue(cost);
                    };
                    if let Some(delta) = cost.delta_usd {
                        *entry -= delta;
                    }
                    *entry <= Decimal::ZERO
                };

                if exhausted {
                    tracing::info!(
                        user_id,
                        session_id = %session_id,
                        total_usd = %cost.total_usd,
                        "quota stub: budget exhausted, cancelling session",
                    );
                    core.cancel_session(session_id, "budget_exhausted".into())
                        .await;
                    return HookResult::Stop(cost);
                }
                HookResult::Continue(cost)
            }
        })?;

        // before_turn: refuse the *next* turn if balance is already
        // ≤ 0. Catches the case where on_cost_tick exhausted the budget
        // but cancellation lost the race with the loop's next iteration.
        let core_for_before = ctx.core();
        let balances_for_before = balances;
        ctx.on_before_turn(Priority::default(), move |before: BeforeTurnCtx| {
            let core = core_for_before.clone();
            let balances = balances_for_before.clone();
            async move {
                let Some(session_id) = before.session_id else {
                    return HookResult::Continue(before);
                };
                let Some(user_id) = user_id_for_session(&core, session_id).await else {
                    return HookResult::Continue(before);
                };
                let exhausted = {
                    let map = balances.lock().expect("balances mutex poisoned");
                    map.get(&user_id)
                        .copied()
                        .map(|b| b <= Decimal::ZERO)
                        .unwrap_or(false)
                };
                if exhausted {
                    tracing::info!(
                        user_id,
                        session_id = %session_id,
                        "quota stub: before_turn refusing — budget exhausted",
                    );
                    return HookResult::Stop(before);
                }
                HookResult::Continue(before)
            }
        })?;

        Ok(())
    }
}
