//! Concrete [`CoreApi`] backed by [`AppState`] handles.
//!
//! Lives in `openlet-server` (not `openlet-core`) because it composes
//! `MemoryStore`, `EventSink`, `ConversationRuntime`, and `Config` —
//! the binary is the right layer to wire those together. Plugins
//! receive `Arc<dyn CoreApi>` inside `install` and clone it into hook
//! closures so they can read session state, record cost, and emit
//! events from any dispatch site.

use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use chrono::Utc;
use dashmap::DashMap;
use openlet_adapters::localfs::SecretRedactor;
use openlet_core::adapters::event_sink::{EventSink, Persistence};
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::config::Config;
use openlet_core::dispatch::{HookChains, dispatch};
use openlet_core::hooks::io::{NotificationCtx, NotificationLevel};
use openlet_core::runtime::ConversationRuntime;
use openlet_core::types::event::{AgentEvent, NotificationLevel as EventNotificationLevel};
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
    /// Late-bound hook chains — required by `emit_notification` to fan
    /// the ctx through registered observer plugins. Bound at boot
    /// alongside `runtime` / `active_turns`.
    pub hook_chains: Arc<OnceLock<Arc<HookChains>>>,
    /// Per-session notification rate limiter buckets. 10 emits/sec
    /// cumulative across plugins. None for global (session-less) emits.
    notif_buckets: Arc<DashMap<SessionId, Arc<NotifBucket>>>,
    notif_redactor: Arc<SecretRedactor>,
}

/// Token bucket: 10 capacity, refill 10/sec. Cumulative across plugins
/// (per-session, not per-plugin) so a single misbehaving plugin can't
/// be hidden by another well-behaved one's quota.
#[derive(Debug)]
struct NotifBucket {
    tokens: AtomicU32,
    last_refill_ms: AtomicI64,
}

impl NotifBucket {
    fn new() -> Self {
        Self {
            tokens: AtomicU32::new(NOTIF_BUCKET_CAPACITY),
            last_refill_ms: AtomicI64::new(Utc::now().timestamp_millis()),
        }
    }

    /// Try to take one token. Refills lazily on each call. Returns
    /// `true` if a token was consumed (allow), `false` if drained
    /// (drop notification).
    fn try_take(&self) -> bool {
        let now_ms = Utc::now().timestamp_millis();
        let last = self.last_refill_ms.load(Ordering::Acquire);
        let elapsed_ms = now_ms.saturating_sub(last);
        if elapsed_ms >= NOTIF_REFILL_INTERVAL_MS {
            // Refill at 10 tokens/sec → cap at capacity.
            let refill = (elapsed_ms / NOTIF_REFILL_INTERVAL_MS).min(i64::from(u32::MAX)) as u32;
            self.last_refill_ms.store(now_ms, Ordering::Release);
            let prev = self.tokens.load(Ordering::Acquire);
            let next = prev.saturating_add(refill).min(NOTIF_BUCKET_CAPACITY);
            self.tokens.store(next, Ordering::Release);
        }
        // CAS decrement.
        loop {
            let cur = self.tokens.load(Ordering::Acquire);
            if cur == 0 {
                return false;
            }
            if self
                .tokens
                .compare_exchange(cur, cur - 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }
}

const NOTIF_BUCKET_CAPACITY: u32 = 10;
/// 100ms per token → 10 tokens/sec. Refill happens lazily on `try_take`.
const NOTIF_REFILL_INTERVAL_MS: i64 = 100;

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
                openlet_core::dispatch::DispatchOutcome::Completed(c)
                | openlet_core::dispatch::DispatchOutcome::Stopped(c) => c,
                openlet_core::dispatch::DispatchOutcome::Denied { .. } => NotificationCtx {
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
mod tests {
    use super::*;

    #[test]
    fn notif_bucket_drops_after_capacity() {
        let bucket = NotifBucket::new();
        // 10 capacity → first 10 succeed.
        for _ in 0..NOTIF_BUCKET_CAPACITY {
            assert!(bucket.try_take(), "should succeed within capacity");
        }
        // 11th in the same instant → drops.
        assert!(
            !bucket.try_take(),
            "11th emit must drop (rate limit triggered)"
        );
    }

    #[test]
    fn notif_bucket_refills_after_interval() {
        let bucket = NotifBucket::new();
        for _ in 0..NOTIF_BUCKET_CAPACITY {
            assert!(bucket.try_take());
        }
        // Force a refill window by rewinding `last_refill_ms` 1 sec.
        bucket
            .last_refill_ms
            .store(Utc::now().timestamp_millis() - 1_000, Ordering::Release);
        // After refill, capacity restores. We don't assert the exact
        // count because the refill formula is `elapsed / interval`;
        // 1 sec / 100ms = 10, capped at capacity.
        assert!(bucket.try_take(), "post-refill emit should succeed");
    }
}
