//! End-to-end tests for `HookedEventSink` — covers the OnEvent dispatch
//! site at the layer where the runtime actually publishes events.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Utc;
use openlet_core::adapters::event_sink::{DeliveredEvent, EventSink, Persistence};
use openlet_core::adapters::hooked_event_sink::HookedEventSink;
use openlet_core::dispatch::{HookChains, HookEntry};
use openlet_core::error::EventError;
use openlet_core::hooks::{HookKind, HookResult, Priority, io::OnEventCtx};
use openlet_core::types::event::{AgentEvent, EventFilter};
use openlet_core::types::message::MessageId;
use openlet_core::types::session::SessionId;
use tokio::sync::broadcast;

#[derive(Default)]
struct CapturingSink {
    captured: Mutex<Vec<AgentEvent>>,
}

#[async_trait]
impl EventSink for CapturingSink {
    async fn publish(&self, ev: AgentEvent, _: Persistence) -> Result<(), EventError> {
        self.captured.lock().unwrap().push(ev);
        Ok(())
    }
    fn subscribe(&self, _: EventFilter) -> broadcast::Receiver<DeliveredEvent> {
        let (_tx, rx) = broadcast::channel(1);
        rx
    }
    async fn replay_since(&self, _: SessionId, _: i64) -> Result<Vec<DeliveredEvent>, EventError> {
        Ok(vec![])
    }
}

fn sample_event() -> AgentEvent {
    AgentEvent::MessageCreated {
        session_id: SessionId::new(),
        message_id: MessageId::new(),
        at: Utc::now(),
    }
}

fn entry<F, Fut>(manifest_id: &str, f: F) -> HookEntry<OnEventCtx>
where
    F: Fn(OnEventCtx) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = HookResult<OnEventCtx>> + Send + 'static,
{
    HookEntry {
        manifest_id: manifest_id.to_string(),
        priority: Priority(50),
        registration_index: 0,
        kind: HookKind::OnEvent,
        func: Arc::new(move |c| Box::pin(f(c))),
    }
}

#[tokio::test]
async fn empty_chain_forwards_event_unchanged() {
    let inner = Arc::new(CapturingSink::default());
    let chains = Arc::new(HookChains::new());
    let sink = HookedEventSink::new(inner.clone(), chains);

    sink.publish(sample_event(), Persistence::Transient)
        .await
        .unwrap();

    assert_eq!(inner.captured.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn continue_forwards_event_unchanged() {
    let inner = Arc::new(CapturingSink::default());
    let mut chains = HookChains::new();
    chains
        .on_event
        .push(entry("noop", |c| async move { HookResult::Continue(c) }));
    let sink = HookedEventSink::new(inner.clone(), Arc::new(chains));

    sink.publish(sample_event(), Persistence::Transient)
        .await
        .unwrap();

    assert_eq!(inner.captured.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn replace_swaps_forwarded_event_payload() {
    let inner = Arc::new(CapturingSink::default());
    let new_session = SessionId::new();

    let mut chains = HookChains::new();
    chains
        .on_event
        .push(entry("redactor", move |mut c| async move {
            c.event = Some(AgentEvent::MessageCreated {
                session_id: new_session,
                message_id: MessageId::new(),
                at: Utc::now(),
            });
            HookResult::Replace(c)
        }));
    let sink = HookedEventSink::new(inner.clone(), Arc::new(chains));

    sink.publish(sample_event(), Persistence::Transient)
        .await
        .unwrap();

    let captured = inner.captured.lock().unwrap();
    assert_eq!(captured.len(), 1);
    match &captured[0] {
        AgentEvent::MessageCreated { session_id, .. } => assert_eq!(*session_id, new_session),
        other => panic!("expected MessageCreated, got {other:?}"),
    }
}

#[tokio::test]
async fn deny_preserves_original_event_for_downstream_observers() {
    // Firehose contract (amendment §4): a Deny outcome MUST NOT silence
    // the inner sink for non-buggy observers. The dispatch_event runner
    // preserves the original event so audit/SSE/replay still receive it.
    let inner = Arc::new(CapturingSink::default());
    let mut chains = HookChains::new();
    chains.on_event.push(entry("blocker", |_c| async move {
        HookResult::Deny {
            reason: "synthetic".into(),
            feedback: None,
        }
    }));
    let sink = HookedEventSink::new(inner.clone(), Arc::new(chains));

    let result = sink.publish(sample_event(), Persistence::Transient).await;
    assert!(result.is_ok(), "Deny must NOT propagate as Err");
    assert_eq!(
        inner.captured.lock().unwrap().len(),
        1,
        "Deny must preserve original event (firehose contract)",
    );
}

#[tokio::test]
async fn stop_still_forwards_event_via_downgrade() {
    // Stop is downgraded to Continue by dispatch_event, so the event
    // still reaches downstream observers (audit / SSE).
    let inner = Arc::new(CapturingSink::default());
    let mut chains = HookChains::new();
    chains
        .on_event
        .push(entry("stopper", |c| async move { HookResult::Stop(c) }));
    let sink = HookedEventSink::new(inner.clone(), Arc::new(chains));

    sink.publish(sample_event(), Persistence::Transient)
        .await
        .unwrap();

    assert_eq!(
        inner.captured.lock().unwrap().len(),
        1,
        "Stop must downgrade so observers still receive the event",
    );
}
