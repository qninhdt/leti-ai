//! Tokio broadcast `EventSink` impl with two-tier publish (§G).
//!
//! `Persistence::Durable` events are written to the `events` table FIRST
//! (so Last-Event-ID exists for SSE resume), then broadcast. The DB write
//! is what makes resume work — broadcast subscribers see the same event.
//!
//! `Persistence::Transient` events (`part.delta`, `heartbeat`) skip the
//! SQLite write entirely; broadcast only.

use async_trait::async_trait;
use openlet_core::adapters::event_sink::{DeliveredEvent, EventSink, Persistence};
use openlet_core::error::EventError;
use openlet_core::types::event::{AgentEvent, EventFilter};
use openlet_core::types::session::SessionId;
use tokio::sync::{Mutex, broadcast};

use crate::sqlite::event_repo::SqliteEventRepo;

/// Capacity of the in-memory broadcast channel. Slow subscribers lag and
/// receive `RecvError::Lagged` rather than blocking the publisher.
const BROADCAST_CAPACITY: usize = 1024;

/// Broadcast-only bus, no durable persistence. Used by tests and any
/// future ephemeral wiring; production boots `BroadcastBus::with_repo`.
pub struct BroadcastBus {
    tx: broadcast::Sender<DeliveredEvent>,
    repo: Option<SqliteEventRepo>,
    /// Serializes the (id-allocate → repo.append_with_id → tx.send) triple
    /// for durable publishes so id assignment, durability, AND broadcast all
    /// happen in the SAME event_id order. The value is the last-assigned
    /// event_id (`None` until the first durable publish lazily seeds it from
    /// `SELECT MAX(id)`).
    ///
    /// WHY all three under one lock (H1): the id MUST be allocated in the
    /// same order it is inserted and sent. If the id were allocated lock-free
    /// before acquiring a send-only lock, a task holding id=N could lose the
    /// race to a task holding id=N+1, inserting/broadcasting N+1 before N —
    /// the exact reorder the original mutex prevented. An LEID-tracking SSE
    /// subscriber advances its cursor by ARRIVAL order and drops live frames
    /// `id <= replay_high_water` (`routes/event.rs`), so an out-of-order
    /// broadcast is lost live AND never replayed. Keeping the slow SQLite
    /// write inside the lock trades a little publish concurrency (SQLite
    /// serializes writers anyway) for a hard zero-drop ordering guarantee.
    ///
    /// SEED (H1): the counter is seeded from the persisted `MAX(id)` (not 0)
    /// because the `events` table survives restarts. A process-local counter
    /// starting at 0 each boot would re-issue ids 1.. and collide with
    /// surviving rows on the explicit-PK insert → `UNIQUE` violation.
    next_event_id: Mutex<Option<i64>>,
}

impl BroadcastBus {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            tx,
            repo: None,
            next_event_id: Mutex::new(None),
        }
    }

    /// Construct a bus that writes durable events to `repo` before
    /// broadcasting. Transient events skip the repo entirely.
    #[must_use]
    pub fn with_repo(repo: SqliteEventRepo) -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            tx,
            repo: Some(repo),
            next_event_id: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn sender(&self) -> broadcast::Sender<DeliveredEvent> {
        self.tx.clone()
    }

    /// Lookup the durable event repo for replay queries (used by
    /// `/v1/event` Last-Event-ID handling).
    #[must_use]
    pub fn repo(&self) -> Option<&SqliteEventRepo> {
        self.repo.as_ref()
    }
}

impl Default for BroadcastBus {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventSink for BroadcastBus {
    /// Two-tier publish per amendment §G:
    ///   - `Durable` → allocate event_id (monotonic, seeded from MAX(id)),
    ///     persist via `append_with_id`, then broadcast — all in event_id order
    ///   - `Transient` → broadcast only
    /// Broadcast `Err` is suppressed: a turn may run with no subscribers.
    async fn publish(&self, ev: AgentEvent, persistence: Persistence) -> Result<(), EventError> {
        if matches!(persistence, Persistence::Durable) {
            if let Some(repo) = &self.repo {
                // Hold the lock across id-allocate + append + send so the
                // event_id order is identical for assignment, durability, and
                // broadcast. See `next_event_id` docs for the ordering proof
                // and the MAX(id) seed rationale.
                let mut guard = self.next_event_id.lock().await;
                // Lazily seed the counter from the persisted high-water mark
                // on first durable publish (after a restart the table already
                // holds rows; starting at 0 would collide on the explicit PK).
                let seed = match *guard {
                    Some(prev) => prev,
                    None => repo.max_event_id().await?,
                };
                let event_id = seed + 1;
                let session_id = session_id_of(&ev);
                // Self-heal on append failure: drop the cached counter so the
                // NEXT durable publish re-seeds from `SELECT MAX(id)`. This
                // covers both a transient IO error (row NOT inserted → MAX(id)
                // still = `seed`, next publish re-issues `seed+1` cleanly) AND
                // the pathological case where the row DID commit but we still
                // observed an error (MAX(id) = `seed+1`, next publish issues
                // `seed+2`, no permanent UNIQUE-collision wedge). NOTE:
                // `publish()` must be awaited to completion — if its future is
                // dropped mid-`append_with_id`, the counter cannot be reset
                // here; the next successful re-seed still recovers, but callers
                // should not race `publish()` under a `select!`/`timeout`.
                if let Err(e) = repo.append_with_id(event_id, session_id, &ev).await {
                    *guard = None;
                    return Err(e);
                }
                // Only advance the counter AFTER the insert succeeds — a
                // failed append must not burn an id (which would leave a gap
                // the SSE replay treats as a permanently-missing event).
                *guard = Some(event_id);
                let _ = self.tx.send(DeliveredEvent {
                    event_id: Some(event_id),
                    event: ev,
                });
                return Ok(());
            }
        }
        // Transient (or durable-without-repo): broadcast only.
        let _ = self.tx.send(DeliveredEvent {
            event_id: None,
            event: ev,
        });
        Ok(())
    }

    fn subscribe(&self, _filter: EventFilter) -> broadcast::Receiver<DeliveredEvent> {
        self.tx.subscribe()
    }

    async fn replay_since(
        &self,
        session_id: SessionId,
        after_id: i64,
    ) -> Result<Vec<DeliveredEvent>, EventError> {
        let Some(repo) = &self.repo else {
            return Ok(Vec::new());
        };
        let rows = repo.list_since(session_id, after_id).await?;
        Ok(rows
            .into_iter()
            .map(|(id, ev)| DeliveredEvent {
                event_id: Some(id),
                event: ev,
            })
            .collect())
    }

    async fn replay_since_global(&self, after_id: i64) -> Result<Vec<DeliveredEvent>, EventError> {
        let Some(repo) = &self.repo else {
            return Ok(Vec::new());
        };
        let rows = repo.list_since_global(after_id).await?;
        Ok(rows
            .into_iter()
            .map(|(id, ev)| DeliveredEvent {
                event_id: Some(id),
                event: ev,
            })
            .collect())
    }
}

/// Extract the session id from an `AgentEvent` so it can be written to
/// the `events.session_id` column for per-session replay queries.
fn session_id_of(ev: &AgentEvent) -> Option<openlet_core::types::session::SessionId> {
    match ev {
        AgentEvent::SessionStatus { session_id, .. }
        | AgentEvent::MessageCreated { session_id, .. }
        | AgentEvent::PartCreated { session_id, .. }
        | AgentEvent::PartDelta { session_id, .. }
        | AgentEvent::PartUpdated { session_id, .. }
        | AgentEvent::StepFinished { session_id, .. }
        | AgentEvent::PermissionAsked { session_id, .. }
        | AgentEvent::PermissionResolved { session_id, .. }
        | AgentEvent::QuestionRequested { session_id, .. }
        | AgentEvent::PlanModeEntered { session_id, .. }
        | AgentEvent::PlanModeExited { session_id, .. }
        | AgentEvent::AttachmentAccepted { session_id, .. } => Some(*session_id),
        AgentEvent::Error { session_id, .. } | AgentEvent::PluginError { session_id, .. } => {
            *session_id
        }
        AgentEvent::SubagentStarted {
            parent_session_id, ..
        }
        | AgentEvent::SubagentOutput {
            parent_session_id, ..
        }
        | AgentEvent::SubagentFinished {
            parent_session_id, ..
        } => Some(*parent_session_id),
        AgentEvent::NotificationEmitted { session_id, .. } => *session_id,
        AgentEvent::Heartbeat => None,
    }
}
