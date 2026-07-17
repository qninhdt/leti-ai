//! [`HookedEventSink`] — wraps an inner [`EventSink`] and runs the
//! `on_event` hook chain before forwarding.
//!
//! The runner is the specialized [`crate::dispatch::dispatch_event`]
//! that downgrades `Stop`/`Deny` to `Continue`, so a buggy plugin can
//! never swallow events for downstream observers (SSE, audit log).
//!
//! `Replace`-mutated events ARE forwarded as-is — observers see the
//! mutated payload. This is by design: secret-redaction plugins land
//! here.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::adapters::event_sink::{DeliveredEvent, EventSink, Persistence};
use crate::dispatch::{HookChains, dispatch_event};
use crate::error::EventError;
use crate::hooks::io::OnEventCtx;
use crate::types::event::{AgentEvent, EventFilter};
use crate::types::session::SessionId;

pub struct HookedEventSink {
    inner: Arc<dyn EventSink>,
    hook_chains: Arc<HookChains>,
}

impl HookedEventSink {
    #[must_use]
    pub fn new(inner: Arc<dyn EventSink>, hook_chains: Arc<HookChains>) -> Self {
        Self { inner, hook_chains }
    }
}

#[async_trait]
impl EventSink for HookedEventSink {
    async fn publish(&self, ev: AgentEvent, persistence: Persistence) -> Result<(), EventError> {
        // Skip dispatch entirely when no plugin registered the chain —
        // O(1) when chain is empty.
        if self.hook_chains.on_event.is_empty() {
            return self.inner.publish(ev, persistence).await;
        }
        let ctx = OnEventCtx { event: Some(ev) };
        let out = dispatch_event(&self.hook_chains.on_event, ctx).await;
        match out.event {
            Some(forwarded) => self.inner.publish(forwarded, persistence).await,
            // Plugin returned a default ctx (e.g. via Deny downgrade);
            // drop the event silently — plugin chose to suppress it.
            None => Ok(()),
        }
    }

    fn subscribe(&self, filter: EventFilter) -> broadcast::Receiver<DeliveredEvent> {
        self.inner.subscribe(filter)
    }

    async fn replay_since(
        &self,
        session_id: SessionId,
        after_id: i64,
    ) -> Result<Vec<DeliveredEvent>, EventError> {
        self.inner.replay_since(session_id, after_id).await
    }

    async fn replay_since_global(&self, after_id: i64) -> Result<Vec<DeliveredEvent>, EventError> {
        // Must forward to the inner sink. The default trait impl returns
        // an empty Vec, so without this override a global (sessionless)
        // SSE resume through the hooked decorator would silently replay
        // nothing — dropping every durable event the client missed.
        // Mirrors the per-session `replay_since` forwarding above.
        self.inner.replay_since_global(after_id).await
    }
}
