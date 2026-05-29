use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::error::EventError;
use crate::types::event::{AgentEvent, EventFilter};
use crate::types::session::SessionId;

/// Whether an event must be persisted to durable storage. Per amendment §G,
/// `part.delta` and `heartbeat` are TRANSIENT (broadcast-only); all other
/// kinds are durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persistence {
    Durable,
    Transient,
}

/// Wraps an `AgentEvent` with the durable autoincrement id assigned at
/// publish time (when the event was persisted to the `events` table).
/// SSE handlers use the id as the `Last-Event-ID` resume cursor.
/// Transient events carry `event_id = None`.
#[derive(Debug, Clone)]
pub struct DeliveredEvent {
    pub event_id: Option<i64>,
    pub event: AgentEvent,
}

/// Publishes domain events. Phase 5 implements `BroadcastBus` with the
/// two-tier publisher (in-memory broadcast + SQLite write for durable).
#[async_trait]
pub trait EventSink: Send + Sync + 'static {
    /// Publish `ev` to durable storage (if `Persistence::Durable` and a
    /// repo is wired) and broadcast to live subscribers.
    ///
    /// Ordering contract: for two `Persistence::Durable` calls A and B
    /// where A returns `Ok` before B starts, every subscriber observes
    /// A's `event_id < B`'s `event_id` AND receives them in the same
    /// order on the broadcast channel. Implementations MUST serialize
    /// the `(repo.append → tx.send)` pair per call so the broadcast
    /// order matches the assigned-id order — otherwise SSE consumers
    /// tracking `Last-Event-ID` on the live channel could skip events.
    ///
    /// `Persistence::Transient` events skip the repo and have no
    /// `event_id`; ordering between transient and durable events is
    /// not guaranteed.
    async fn publish(&self, ev: AgentEvent, persistence: Persistence) -> Result<(), EventError>;

    /// Returns a fresh broadcast receiver. The caller filters as it reads.
    fn subscribe(&self, filter: EventFilter) -> broadcast::Receiver<DeliveredEvent>;

    /// Replay durable events for `session_id` with id strictly greater
    /// than `after_id`. Default impl returns empty (test stubs can opt
    /// out of replay support).
    async fn replay_since(
        &self,
        _session_id: SessionId,
        _after_id: i64,
    ) -> Result<Vec<DeliveredEvent>, EventError> {
        Ok(Vec::new())
    }

    /// Global replay (no session filter). Used by the global SSE channel
    /// when `Last-Event-ID` is present without a session filter.
    /// Default impl returns empty for stubs.
    async fn replay_since_global(&self, _after_id: i64) -> Result<Vec<DeliveredEvent>, EventError> {
        Ok(Vec::new())
    }
}
