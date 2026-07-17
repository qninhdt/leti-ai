//! Plugin-fault observability: a synthetic deny (panic/timeout) must land
//! a durable `PluginError` event on the event log via
//! `publish_fault_if_any`, so cloud operators can grep `kind=plugin.error`
//! without parsing logs.
//!
//! Layer: integration. Real `BroadcastBus` + on-disk-shaped sqlite repo
//! (in-memory pool), real dispatch fault path. Complements
//! `dispatch_runner_panic_isolation.rs` (which proves the Denied{fault}
//! synthesis) by proving the fault is also PUBLISHED durably — previously
//! untested.

use std::sync::Arc;

use leti_adapters::bus::BroadcastBus;
use leti_adapters::sqlite::event_repo::SqliteEventRepo;
use leti_adapters::sqlite::open_in_memory;
use leti_core::adapters::event_sink::EventSink;
use leti_core::dispatch::{DispatchOutcome, FaultKind, PluginFault, publish_fault_if_any};
use leti_core::hooks::HookKind;
use leti_core::types::event::AgentEvent;

#[tokio::test]
async fn synthetic_fault_publishes_durable_plugin_error() {
    let pool = open_in_memory().await.expect("sqlite");
    let bus = BroadcastBus::with_repo(SqliteEventRepo::new(pool.clone()));
    let events: Arc<dyn EventSink> = Arc::new(bus);

    // A timeout fault from a before_turn hook — the shape the runtime
    // synthesizes when a plugin hook exceeds its budget.
    let outcome: DispatchOutcome<()> = DispatchOutcome::Denied {
        reason: "hook timed out".into(),
        feedback: None,
        plugin_fault: Some(PluginFault {
            plugin_id: "slow-plugin".into(),
            hook: HookKind::BeforeTurn,
            kind: FaultKind::Timeout,
            message: "exceeded 5s budget".into(),
        }),
    };

    publish_fault_if_any(&events, None, &outcome).await;

    // The event must be durably persisted (global replay returns it).
    let replayed = events.replay_since_global(0).await.expect("replay");
    let found = replayed.iter().find_map(|d| match &d.event {
        AgentEvent::PluginError {
            plugin_id,
            hook,
            message,
            ..
        } => Some((plugin_id.clone(), hook.clone(), message.clone())),
        _ => None,
    });

    let (plugin_id, hook, message) =
        found.expect("a durable PluginError event must be published for a synthetic fault");
    assert_eq!(plugin_id, "slow-plugin");
    // `hook` is the stable `{hook}|{kind}` label (HookKind::as_str form).
    assert_eq!(hook, "before_turn|timeout");
    assert!(message.contains("5s budget"));
}

#[tokio::test]
async fn explicit_deny_without_fault_publishes_nothing() {
    // A plugin's OWN `Deny` return (no panic/timeout) carries no
    // `plugin_fault`, so it must NOT spam the durable log — only synthetic
    // faults do. Guards against turning every policy deny into an error.
    let pool = open_in_memory().await.expect("sqlite");
    let bus = BroadcastBus::with_repo(SqliteEventRepo::new(pool.clone()));
    let events: Arc<dyn EventSink> = Arc::new(bus);

    let outcome: DispatchOutcome<()> = DispatchOutcome::Denied {
        reason: "policy denied".into(),
        feedback: Some("not allowed".into()),
        plugin_fault: None,
    };
    publish_fault_if_any(&events, None, &outcome).await;

    let replayed = events.replay_since_global(0).await.expect("replay");
    assert!(
        replayed.is_empty(),
        "a fault-free deny must not publish a PluginError, got {} events",
        replayed.len()
    );
}
