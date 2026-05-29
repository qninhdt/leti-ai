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
    /// Serializes the (repo.append → tx.send) pair for durable publishes
    /// so subscribers observe `event_id` monotonically. Without this,
    /// task A could write event_id=1 to SQLite, get preempted before
    /// `tx.send`, task B writes event_id=2 + sends, then A sends 1 —
    /// any LEID-tracking subscriber would skip 1.
    durable_publish: Mutex<()>,
}

impl BroadcastBus {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            tx,
            repo: None,
            durable_publish: Mutex::new(()),
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
            durable_publish: Mutex::new(()),
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
    ///   - `Durable` → repo.append (assigns Last-Event-ID), then broadcast
    ///   - `Transient` → broadcast only
    /// Broadcast `Err` is suppressed: a turn may run with no subscribers.
    async fn publish(&self, ev: AgentEvent, persistence: Persistence) -> Result<(), EventError> {
        if matches!(persistence, Persistence::Durable) {
            if let Some(repo) = &self.repo {
                // Hold the mutex across append + send so concurrent
                // durable publishes broadcast in the SAME order they
                // received event_ids. Releases before any await on the
                // subscriber side; broadcast.send is sync so this
                // critical section is bounded by the SQLite write.
                let _g = self.durable_publish.lock().await;
                let session_id = session_id_of(&ev);
                let event_id = repo.append(session_id, &ev).await?;
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
