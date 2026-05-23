//! Tokio broadcast `EventSink` impl.
//!
//! Phase 1 stub. Phase 5 implements the two-tier publisher (§G):
//! durable kinds → SQLite + broadcast; transient kinds → broadcast only.

use async_trait::async_trait;
use openlet_core::adapters::event_sink::{EventSink, Persistence};
use openlet_core::error::EventError;
use openlet_core::types::event::{AgentEvent, EventFilter};
use tokio::sync::broadcast;

/// Capacity of the in-memory broadcast channel. Slow subscribers lag and
/// receive `RecvError::Lagged` rather than blocking the publisher.
const BROADCAST_CAPACITY: usize = 1024;

pub struct BroadcastBus {
    tx: broadcast::Sender<AgentEvent>,
}

impl BroadcastBus {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self { tx }
    }

    #[must_use]
    pub fn sender(&self) -> broadcast::Sender<AgentEvent> {
        self.tx.clone()
    }
}

impl Default for BroadcastBus {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventSink for BroadcastBus {
    // TODO(phase-5): implement two-tier publish per §G.
    // - durable kinds → SQLite write + broadcast::Sender::send
    // - transient kinds (`part.delta`, `heartbeat`) → broadcast only
    // Until then, returning Unimplemented makes "no events ever fire"
    // surface as a real error rather than silent staleness.
    async fn publish(
        &self,
        _ev: AgentEvent,
        _persistence: Persistence,
    ) -> Result<(), EventError> {
        Err(EventError::Unimplemented)
    }

    fn subscribe(&self, _filter: EventFilter) -> broadcast::Receiver<AgentEvent> {
        self.tx.subscribe()
    }
}
