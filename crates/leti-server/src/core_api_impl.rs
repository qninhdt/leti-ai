//! Concrete [`CoreApi`] backed by [`AppState`] handles.
//!
//! Lives in `leti-server` (not `leti-core`) because it composes
//! `MemoryStore`, `EventSink`, `ConversationRuntime`, and `Config` —
//! the binary is the right layer to wire those together. Plugins
//! receive `Arc<dyn CoreApi>` inside `install` and clone it into hook
//! closures so they can read session state, record cost, and emit
//! events from any dispatch site.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use dashmap::DashMap;
use leti_adapters::localfs::SecretRedactor;
use leti_core::adapters::event_sink::{EventSink, Persistence};
use leti_core::adapters::memory_store::MemoryStore;
use leti_core::config::Config;
use leti_core::dispatch::{HookChains, dispatch};
use leti_core::hooks::io::{NotificationCtx, NotificationLevel};
use leti_core::runtime::ConversationRuntime;
use leti_core::types::event::{AgentEvent, NotificationLevel as EventNotificationLevel};
use leti_core::types::session::{SessionId, SessionMeta, SessionStatus};
use leti_plugin_api::context::CoreApi;
use rust_decimal::Decimal;

use crate::app_state::TurnHandle;
use crate::events::publish_status;
use crate::notif_bucket::NotifBucket;

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
    /// Late-bound hook chains — required by `emit_notification` to fan
    /// the ctx through registered observer plugins. Bound at boot
    /// alongside `runtime` / `active_turns`.
    pub hook_chains: Arc<OnceLock<Arc<HookChains>>>,
    /// Per-session notification rate limiter buckets. 10 emits/sec
    /// cumulative across plugins. None for global (session-less) emits.
    notif_buckets: Arc<DashMap<SessionId, Arc<NotifBucket>>>,
    notif_redactor: Arc<SecretRedactor>,
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
            hook_chains: Arc::new(OnceLock::new()),
            notif_buckets: Arc::new(DashMap::new()),
            notif_redactor: Arc::new(SecretRedactor::default()),
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

    /// Fill the late-bound hook chains. Used by `emit_notification` to
    /// fan the ctx through observer plugins.
    pub fn set_hook_chains(&self, chains: Arc<HookChains>) {
        let _ = self.hook_chains.set(chains);
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
        match key {
            "default_model" => Ok(serde_json::Value::String(self.config.default_model.clone())),
            "bind_addr" => Ok(serde_json::Value::String(self.config.bind_addr.clone())),
            other => Err(format!("unknown config key: {other}")),
        }
    }

    async fn cancel_session(&self, session_id: SessionId, reason: String) {
        // CAS gate so concurrent abort/DELETE/cancel_session emit exactly
        // one Cancelling event. Don't remove the slot — driving task
        // removes its own on exit (closes stale-finalizer race).
        let mut emitted = false;
        if let Some(active) = self.active_turns.get() {
            if let Some(handle) = active.get(&session_id).map(|h| h.clone())
                && handle.request_cancel()
            {
                handle.cancel.cancel();
                emitted = true;
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
        publish_status(&self.events, session_id, SessionStatus::Cancelling).await;
    }

    async fn emit_notification(
        &self,
        session_id: Option<SessionId>,
        level: NotificationLevel,
        title: String,
        body: String,
        plugin_id: String,
    ) {
        // Per-session cumulative rate-limit (10/sec). Session-less emits
        // bypass the limit because the bucket map is keyed by SessionId
        // and there's no obvious "global session" key — operators can
        // still gate session-less floods via tracing.
        if let Some(sid) = session_id {
            let bucket = self
                .notif_buckets
                .entry(sid)
                .or_insert_with(|| Arc::new(NotifBucket::new()))
                .clone();
            if !bucket.try_take() {
                tracing::warn!(
                    session_id = %sid,
                    plugin_id = %plugin_id,
                    "notification rate limit exceeded; dropping notification"
                );
                return;
            }
        }

        // Defense-in-depth: redact `body` BEFORE running the chain so
        // observer plugins never see un-redacted secrets either. Title
        // is shown to user prominently and is plugin-supplied (not
        // model-supplied), so we leave it untouched — plugins control
        // their own title strings.
        let mut body_value = serde_json::Value::String(body.clone());
        self.notif_redactor.redact_in_place(&mut body_value);
        let body_redacted = match body_value {
            serde_json::Value::String(s) => s,
            _ => body,
        };

        // Fan through observer plugins. Notification chain runs after
        // emission — observers cannot suppress (chain is best-effort
        // only, like OnEvent firehose).
        let ctx_in = NotificationCtx {
            session_id,
            level,
            title: title.clone(),
            body: body_redacted.clone(),
            source_plugin: plugin_id.clone(),
        };
        let final_ctx = if let Some(chains) = self.hook_chains.get() {
            match dispatch(&chains.notification, ctx_in).await {
                leti_core::dispatch::DispatchOutcome::Completed(c)
                | leti_core::dispatch::DispatchOutcome::Stopped(c) => c,
                leti_core::dispatch::DispatchOutcome::Denied { .. } => NotificationCtx {
                    session_id,
                    level,
                    title,
                    body: body_redacted,
                    source_plugin: plugin_id,
                },
            }
        } else {
            NotificationCtx {
                session_id,
                level,
                title,
                body: body_redacted,
                source_plugin: plugin_id,
            }
        };

        let event_level = match final_ctx.level {
            NotificationLevel::Info => EventNotificationLevel::Info,
            NotificationLevel::Warn => EventNotificationLevel::Warn,
            NotificationLevel::Error => EventNotificationLevel::Error,
        };

        let _ = self
            .events
            .publish(
                AgentEvent::NotificationEmitted {
                    session_id: final_ctx.session_id,
                    level: event_level,
                    title: final_ctx.title,
                    body: final_ctx.body,
                    plugin_id: final_ctx.source_plugin,
                },
                Persistence::Durable,
            )
            .await;
    }
}

#[cfg(test)]
mod tests {}
