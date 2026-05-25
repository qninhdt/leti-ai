//! Concrete [`CoreApi`] backed by [`AppState`] handles.
//!
//! Lives in `openlet-server` (not `openlet-core`) because it composes
//! `MemoryStore`, `EventSink`, `ConversationRuntime`, and `Config` —
//! the binary is the right layer to wire those together. Plugins
//! receive `Arc<dyn CoreApi>` inside `install` and clone it into hook
//! closures so they can read session state, record cost, and emit
//! events from any dispatch site.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;
use openlet_core::adapters::event_sink::{EventSink, Persistence};
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::config::Config;
use openlet_core::runtime::ConversationRuntime;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::session::{SessionId, SessionMeta, SessionStatus};
use openlet_plugin_api::context::CoreApi;
use rust_decimal::Decimal;

use crate::app_state::TurnHandle;

pub struct CoreApiImpl {
    pub memory: Arc<dyn MemoryStore>,
    pub events: Arc<dyn EventSink>,
    /// Late-bound: the runtime is constructed AFTER `install_plugins`
    /// (because plugins can register a provider the runtime consumes).
    /// `set_runtime` is called once between install and the first turn,
    /// so by the time any hook fires the OnceLock is filled.
    pub runtime: Arc<OnceLock<Arc<ConversationRuntime>>>,
    pub config: Arc<Config>,
    /// Same `active_turns` map AppState holds — sharing the Arc lets
    /// `cancel_session` trip the per-session token without going through
    /// the HTTP layer. Late-bound for the same reason as `runtime`:
    /// AppState is built after `install_plugins`.
    pub active_turns: Arc<OnceLock<Arc<DashMap<SessionId, TurnHandle>>>>,
}

impl CoreApiImpl {
    #[must_use]
    pub fn new(
        memory: Arc<dyn MemoryStore>,
        events: Arc<dyn EventSink>,
        config: Arc<Config>,
    ) -> Self {
        Self {
            memory,
            events,
            runtime: Arc::new(OnceLock::new()),
            config,
            active_turns: Arc::new(OnceLock::new()),
        }
    }

    /// Fill the late-bound runtime handle. Idempotent — re-setting is a
    /// no-op (caller should only call this once at boot).
    pub fn set_runtime(&self, runtime: Arc<ConversationRuntime>) {
        let _ = self.runtime.set(runtime);
    }

    /// Fill the late-bound active_turns map. Same semantics as
    /// [`Self::set_runtime`] — call once at boot, after AppState is
    /// built.
    pub fn set_active_turns(&self, active_turns: Arc<DashMap<SessionId, TurnHandle>>) {
        let _ = self.active_turns.set(active_turns);
    }
}

#[async_trait]
impl CoreApi for CoreApiImpl {
    async fn current_session_meta(&self, session_id: SessionId) -> Option<SessionMeta> {
        // Soft-deleted sessions surface as `None` — memory store filters.
        self.memory.get_session(session_id).await.ok().flatten()
    }

    fn session_cost(&self, session_id: SessionId) -> Decimal {
        self.runtime
            .get()
            .map(|rt| rt.session_cost(session_id))
            .unwrap_or_default()
    }

    fn record_cost(&self, session_id: SessionId, delta: Decimal) {
        if let Some(rt) = self.runtime.get() {
            rt.add_session_cost_external(session_id, delta);
        } else {
            tracing::warn!(
                session_id = %session_id,
                "CoreApi::record_cost called before runtime bound; cost dropped"
            );
        }
    }

    async fn emit_event(&self, event: AgentEvent, persistence: Persistence) {
        let _ = self.events.publish(event, persistence).await;
    }

    fn read_config(&self, key: &str) -> Result<serde_json::Value, String> {
        // Phase 7: max_cost_per_session_usd removed; cost cap is plugin-only.
        // Plugins that need a cap should track it themselves (see test-quota-stub).
        match key {
            "default_model" => Ok(serde_json::Value::String(self.config.default_model.clone())),
            "bind_addr" => Ok(serde_json::Value::String(self.config.bind_addr.clone())),
            other => Err(format!("unknown config key: {other}")),
        }
    }

    async fn cancel_session(&self, session_id: SessionId, reason: String) {
        // Use the CAS gate so concurrent abort/DELETE/cancel_session
        // emit exactly one Cancelling event (closes C6-server). Don't
        // remove the slot — driving task removes its own on exit
        // (closes C1-server stale-finalizer race).
        let mut emitted = false;
        if let Some(active) = self.active_turns.get() {
            if let Some(handle) = active.get(&session_id).map(|h| h.clone()) {
                if handle.request_cancel() {
                    handle.cancel.cancel();
                    emitted = true;
                }
            }
        } else {
            tracing::warn!(
                session_id = %session_id,
                "CoreApi::cancel_session called before active_turns bound"
            );
        }

        if !emitted {
            // Concurrent canceller already emitted the event. No-op so
            // we don't double-publish.
            return;
        }

        if let Err(err) = self
            .memory
            .update_status(session_id, SessionStatus::Cancelling, &reason)
            .await
        {
            tracing::warn!(
                session_id = %session_id,
                error = %err,
                "cancel_session: status write failed"
            );
        }
        let _ = self
            .events
            .publish(
                AgentEvent::SessionStatus {
                    session_id,
                    status: SessionStatus::Cancelling,
                    at: Utc::now(),
                },
                Persistence::Durable,
            )
            .await;
    }
}
