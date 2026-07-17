//! Slice 3a tests for the typed hook dispatch core.
//!
//! Three behaviors locked in here so slice 3b (real dispatch sites) can
//! rely on them: canonical ordering, short-circuit on Stop/Deny, and
//! construction-panic isolation.

use std::sync::Arc;

use leti_plugin_api::dispatch::{DispatchOutcome, FaultKind, HookEntry, HookFuture, dispatch};
use leti_plugin_api::hooks::{HookKind, HookResult, Priority};

#[derive(Debug, Default)]
struct Trace {
    visited: Vec<String>,
}

fn entry<F, Fut>(
    manifest_id: &str,
    priority: u8,
    registration_index: usize,
    f: F,
) -> HookEntry<Trace>
where
    F: Fn(Trace) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = HookResult<Trace>> + Send + 'static,
{
    HookEntry {
        manifest_id: manifest_id.to_string(),
        priority: Priority(priority),
        registration_index,
        kind: HookKind::BeforeTurn,
        func: Arc::new(move |t| Box::pin(f(t)) as HookFuture<Trace>),
    }
}

fn record(
    id: &'static str,
) -> impl Fn(
    Trace,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = HookResult<Trace>> + Send + 'static>>
+ Send
+ Sync
+ 'static {
    move |mut t: Trace| {
        Box::pin(async move {
            t.visited.push(id.to_string());
            HookResult::Continue(t)
        })
    }
}

#[tokio::test]
async fn hook_chain_orders_priority_desc_then_manifest_asc_then_registration_asc() {
    let mut chain: Vec<HookEntry<Trace>> = vec![
        // Same priority — manifest_id breaks the tie (alpha < bravo).
        entry("bravo", 50, 0, record("bravo-50-0")),
        entry("alpha", 50, 1, record("alpha-50-1")),
        // Higher priority leads.
        entry("zulu", 90, 2, record("zulu-90")),
        // Lowest priority trails.
        entry("alpha", 10, 3, record("alpha-10")),
        // Same priority + same manifest — registration index asc.
        entry("alpha", 50, 4, record("alpha-50-4")),
    ];

    // Match HookChains::sort_all canonical order.
    chain.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.manifest_id.cmp(&b.manifest_id))
            .then_with(|| a.registration_index.cmp(&b.registration_index))
    });

    let outcome = dispatch(&chain, Trace::default()).await;
    let trace = match outcome {
        DispatchOutcome::Completed(t) => t,
        other => panic!("expected Completed, got {other:?}"),
    };

    assert_eq!(
        trace.visited,
        vec![
            "zulu-90",
            "alpha-50-1",
            "alpha-50-4",
            "bravo-50-0",
            "alpha-10",
        ],
    );
}

#[tokio::test]
async fn stop_short_circuits_chain_with_terminal_value() {
    let chain = vec![
        entry("a", 80, 0, record("a")),
        entry("b", 50, 1, |mut t: Trace| {
            t.visited.push("b-stop".to_string());
            async move { HookResult::Stop(t) }
        }),
        entry("c", 10, 2, record("c-not-run")),
    ];

    let outcome = dispatch(&chain, Trace::default()).await;
    match outcome {
        DispatchOutcome::Stopped(t) => {
            assert_eq!(t.visited, vec!["a", "b-stop"]);
        }
        other => panic!("expected Stopped, got {other:?}"),
    }
}

#[tokio::test]
async fn deny_short_circuits_chain_with_reason_and_feedback() {
    let chain = vec![
        entry("a", 80, 0, record("a")),
        entry("b", 50, 1, |_t: Trace| async move {
            HookResult::Deny {
                reason: "policy violation".to_string(),
                feedback: Some("retry without secrets".to_string()),
            }
        }),
        entry("c", 10, 2, record("c-not-run")),
    ];

    let outcome = dispatch(&chain, Trace::default()).await;
    match outcome {
        DispatchOutcome::Denied {
            reason, feedback, ..
        } => {
            assert_eq!(reason, "policy violation");
            assert_eq!(feedback.as_deref(), Some("retry without secrets"));
        }
        other => panic!("expected Denied, got {other:?}"),
    }
}

#[tokio::test]
async fn replace_threads_payload_to_next_hook() {
    let chain = vec![
        entry("a", 90, 0, record("a")),
        entry("b", 50, 1, |mut t: Trace| {
            t.visited.push("b-replace".to_string());
            async move { HookResult::Replace(t) }
        }),
        entry("c", 10, 2, record("c-after-replace")),
    ];

    let outcome = dispatch(&chain, Trace::default()).await;
    match outcome {
        DispatchOutcome::Completed(t) => {
            assert_eq!(t.visited, vec!["a", "b-replace", "c-after-replace"]);
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[tokio::test]
async fn polling_panic_is_isolated_and_reported_as_denied() {
    let chain = vec![
        entry("a", 80, 0, record("a")),
        entry("b-poll-panicker", 50, 1, |_t: Trace| async move {
            // Panic inside the future body — surfaces during polling,
            // not construction. Must still be caught + Denied.
            panic!("synthetic polling panic");
            #[allow(unreachable_code)]
            HookResult::Continue(Trace::default())
        }),
        entry("c", 10, 2, record("c-not-run")),
    ];

    let outcome = dispatch(&chain, Trace::default()).await;
    match outcome {
        DispatchOutcome::Denied {
            reason,
            feedback,
            plugin_fault,
        } => {
            assert!(
                reason.contains("b-poll-panicker") && reason.contains("awaiting"),
                "unexpected reason: {reason}"
            );
            assert!(feedback.is_none());
            let fault = plugin_fault.expect("poll panic must carry plugin_fault");
            assert_eq!(fault.plugin_id, "b-poll-panicker");
            assert_eq!(fault.kind, FaultKind::PollPanic);
            assert_eq!(fault.hook, HookKind::BeforeTurn);
        }
        other => panic!("expected Denied (poll panic isolated), got {other:?}"),
    }
}

#[tokio::test]
async fn construction_panic_is_isolated_and_reported_as_denied() {
    let chain = vec![
        entry("a", 80, 0, record("a")),
        entry("b-panicker", 50, 1, |_t: Trace| -> std::pin::Pin<
            Box<dyn std::future::Future<Output = HookResult<Trace>> + Send>,
        > {
            // Panic during closure body, before returning a future.
            panic!("synthetic construction panic");
        }),
        entry("c", 10, 2, record("c-not-run")),
    ];

    let outcome = dispatch(&chain, Trace::default()).await;
    match outcome {
        DispatchOutcome::Denied {
            reason,
            feedback,
            plugin_fault,
        } => {
            assert!(
                reason.contains("b-panicker") && reason.contains("panicked"),
                "unexpected reason: {reason}"
            );
            assert!(feedback.is_none());
            let fault = plugin_fault.expect("construction panic must carry plugin_fault");
            assert_eq!(fault.plugin_id, "b-panicker");
            assert_eq!(fault.kind, FaultKind::ConstructionPanic);
            assert_eq!(fault.hook, HookKind::BeforeTurn);
        }
        other => panic!("expected Denied (panic isolated), got {other:?}"),
    }
}
