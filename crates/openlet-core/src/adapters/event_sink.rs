use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::error::EventError;
use crate::types::event::{AgentEvent, EventFilter};
use crate::types::session::SessionId;

/// Whether an event must be persisted to durable storage.
/// `part.delta` and `heartbeat` are TRANSIENT (broadcast-only); all other
/// kinds are durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persistence {
    Durable,
    Transient,
}

/// Where a cloud event bus should route an event. The local in-process
/// broadcast bus ignores this (every subscriber sees every event); a
/// Kafka/Redis impl uses it to partition per workspace/user so a tenant
/// only receives its own stream. All fields optional — `None` means
/// "unrouted / broadcast to all", which is the local behavior.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingKey {
    pub workspace: Option<String>,
    pub user: Option<String>,
}

/// Delivery guarantee a sink offers. The local broadcast bus is
/// best-effort (a lagging subscriber drops frames); a durable cloud bus
/// can offer at-least-once. Consumers that need exactly-once dedupe on
/// `event_id` regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliverySemantics {
    /// Frames may be dropped to a slow subscriber (in-proc broadcast).
    BestEffort,
    /// Every event is delivered at least once (durable cloud bus); the
    /// consumer must dedupe on `event_id`.
    AtLeastOnce,
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

/// Publishes domain events via a two-tier publisher
/// (in-memory broadcast + SQLite write for durable).
#[async_trait]
pub trait EventSink: Send + Sync + 'static {
    /// Publish `ev` to durable storage (if `Persistence::Durable` and a
    /// repo is wired) and broadcast to live subscribers.
    ///
    /// Ordering contract: for two `Persistence::Durable` calls A and B
    /// where A returns `Ok` before B starts, every subscriber observes
    /// A's `event_id < B`'s `event_id` AND receives them in the same
    /// order on the broadcast channel. `event_id` is monotonically
    /// assigned at publish time AND broadcast in `event_id` order; replay
    /// via `event_id` ordering is authoritative. Implementations MUST
    /// serialize the `(allocate event_id → persist → tx.send)` triple per
    /// call so the broadcast order matches the assigned-id order —
    /// otherwise SSE consumers tracking `Last-Event-ID` on the live
    /// channel could skip events (a frame broadcast out of order is
    /// dropped live and never replayed).
    ///
    /// `Persistence::Transient` events skip the repo and have no
    /// `event_id`; ordering between transient and durable events is
    /// not guaranteed.
    async fn publish(&self, ev: AgentEvent, persistence: Persistence) -> Result<(), EventError>;

    /// Publish with an explicit routing key. The default ignores the key
    /// and delegates to [`Self::publish`] (the local broadcast bus
    /// fans out to every subscriber). A cloud bus overrides this to
    /// partition delivery per workspace/user.
    async fn publish_routed(
        &self,
        ev: AgentEvent,
        persistence: Persistence,
        _routing: RoutingKey,
    ) -> Result<(), EventError> {
        self.publish(ev, persistence).await
    }

    /// The delivery guarantee this sink offers. Default is
    /// [`DeliverySemantics::BestEffort`] (the in-process broadcast bus);
    /// a durable cloud bus overrides to [`DeliverySemantics::AtLeastOnce`].
    fn delivery_semantics(&self) -> DeliverySemantics {
        DeliverySemantics::BestEffort
    }

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
