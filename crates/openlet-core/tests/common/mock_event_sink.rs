//! `RecordingEventSink` — captures every `publish` call into a vec for
//! assertions, while still broadcasting to subscribers so the runtime
//! sees the same wire shape it does in production.

use std::sync::Mutex;

use async_trait::async_trait;
use openlet_core::adapters::event_sink::{DeliveredEvent, EventSink, Persistence};
use openlet_core::error::EventError;
use openlet_core::types::event::{AgentEvent, EventFilter};
use tokio::sync::broadcast;

pub struct RecordingEventSink {
    tx: broadcast::Sender<DeliveredEvent>,
    captured: Mutex<Vec<(AgentEvent, Persistence)>>,
}

impl RecordingEventSink {
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            tx,
            captured: Mutex::new(Vec::new()),
        }
    }

    /// Drain the captured events. Subsequent calls return only events
    /// captured after the previous drain.
    pub fn take(&self) -> Vec<(AgentEvent, Persistence)> {
        std::mem::take(&mut *self.captured.lock().unwrap())
    }

    /// Snapshot the captured events without draining.
    pub fn snapshot(&self) -> Vec<(AgentEvent, Persistence)> {
        self.captured.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.captured.lock().unwrap().len()
    }
}

impl Default for RecordingEventSink {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EventSink for RecordingEventSink {
    async fn publish(&self, ev: AgentEvent, persistence: Persistence) -> Result<(), EventError> {
        self.captured
            .lock()
            .unwrap()
            .push((ev.clone(), persistence));
        let _ = self.tx.send(DeliveredEvent {
            event_id: None,
            event: ev,
        });
        Ok(())
    }

    fn subscribe(&self, _filter: EventFilter) -> broadcast::Receiver<DeliveredEvent> {
        self.tx.subscribe()
    }
}
