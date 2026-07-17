//! CP4 telemetry: metric emits fire at canonical points with the right
//! labels — and crucially WITHOUT a per-`workspace` label on the open
//! scrape (M16: that label set enumerates tenants).
//!
//! Uses a thread-local `DebuggingRecorder` (`with_local_recorder`) so the
//! assertion is deterministic and parallel-safe — no global install-once
//! recorder, no env mutation.

use std::sync::Arc;

use leti_core::adapters::event_sink::{EventSink, Persistence};
use leti_core::dispatch::{DispatchOutcome, FaultKind, PluginFault, publish_fault_if_any};
use leti_core::error::EventError;
use leti_core::hooks::HookKind;
use leti_core::types::event::{AgentEvent, EventFilter};
use metrics_util::debugging::DebuggingRecorder;

/// No-op sink — `publish_fault_if_any` needs an `EventSink` but the test
/// only cares about the metric emit, not the published event.
struct NoopSink;

#[async_trait::async_trait]
impl EventSink for NoopSink {
    async fn publish(&self, _ev: AgentEvent, _p: Persistence) -> Result<(), EventError> {
        Ok(())
    }
    fn subscribe(
        &self,
        _filter: EventFilter,
    ) -> tokio::sync::broadcast::Receiver<leti_core::adapters::event_sink::DeliveredEvent> {
        let (_tx, rx) = tokio::sync::broadcast::channel(1);
        rx
    }
}

#[test]
fn plugin_fault_emits_counter_with_hook_label_and_no_workspace_label() {
    let recorder = DebuggingRecorder::new();
    let snapshotter = recorder.snapshotter();

    // `with_local_recorder` takes a sync closure; drive the async emit on
    // a current-thread runtime inside it so the thread-local recorder is
    // active for the duration of the call.
    metrics::with_local_recorder(&recorder, || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let events: Arc<dyn EventSink> = Arc::new(NoopSink);
            let outcome: DispatchOutcome<()> = DispatchOutcome::Denied {
                reason: "boom".into(),
                feedback: None,
                plugin_fault: Some(PluginFault {
                    plugin_id: "p1".into(),
                    hook: HookKind::BeforeTurn,
                    kind: FaultKind::Timeout,
                    message: "timed out".into(),
                }),
            };
            publish_fault_if_any(&events, None, &outcome).await;
        });
    });

    let snapshot = snapshotter.snapshot().into_vec();
    let fault = snapshot
        .iter()
        .find(|(ck, _, _, _)| ck.key().name() == "leti_plugin_faults_total")
        .expect("plugin fault counter must be emitted on a faulting deny");

    let labels: Vec<(&str, &str)> = fault
        .0
        .key()
        .labels()
        .map(|l| (l.key(), l.value()))
        .collect();

    assert!(
        labels.iter().any(|(k, _)| *k == "hook"),
        "fault counter must carry a `hook` label, got {labels:?}"
    );
    assert!(
        !labels.iter().any(|(k, _)| *k == "workspace"),
        "M16: the open scrape must NOT carry a per-workspace label, got {labels:?}"
    );
}
