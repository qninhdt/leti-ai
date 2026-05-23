use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::error::EventError;
use crate::types::event::{AgentEvent, EventFilter};

/// Whether an event must be persisted to durable storage. Per amendment §G,
/// `part.delta` and `heartbeat` are TRANSIENT (broadcast-only); all other
/// kinds are durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persistence {
    Durable,
    Transient,
}

/// Publishes domain events. Phase 5 implements `BroadcastBus` with the
/// two-tier publisher (in-memory broadcast + SQLite write for durable).
#[async_trait]
pub trait EventSink: Send + Sync + 'static {
    async fn publish(
        &self,
        ev: AgentEvent,
        persistence: Persistence,
    ) -> Result<(), EventError>;

    /// Returns a fresh broadcast receiver. The caller filters as it reads.
    fn subscribe(&self, filter: EventFilter) -> broadcast::Receiver<AgentEvent>;
}
