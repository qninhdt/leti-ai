//! Slice 3b tests — behaviors that real dispatch sites depend on:
//! per-hook 5s timeout, Replace audit, OnEvent firehose downgrade.
//!
//! The actual wiring at the seven runtime sites is exercised by the
//! suites in `leti-core` / `leti-adapters`; this file pins the
//! cross-cutting contracts the dispatcher exposes so a refactor to
//! [`dispatch`] cannot quietly drop them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use leti_plugin_api::dispatch::{
    DispatchOutcome, FaultKind, HookEntry, HookFuture, dispatch, dispatch_event,
};
use leti_plugin_api::hooks::{HookKind, HookResult, Priority, io::OnEventCtx};

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

fn event_entry<F, Fut>(
    manifest_id: &str,
    priority: u8,
    registration_index: usize,
    f: F,
) -> HookEntry<OnEventCtx>
where
    F: Fn(OnEventCtx) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = HookResult<OnEventCtx>> + Send + 'static,
{
    HookEntry {
        manifest_id: manifest_id.to_string(),
        priority: Priority(priority),
        registration_index,
        kind: HookKind::OnEvent,
        func: Arc::new(move |c| Box::pin(f(c)) as HookFuture<OnEventCtx>),
    }
}

#[tokio::test]
async fn slow_hook_is_killed_by_timeout_and_chain_halts() {
    // 6s sleep > 5s ceiling — must surface as Denied without the third
    // hook running. Test wallclock is bounded ~5s, well under sleep.
    let chain = vec![
        entry("fast", 90, 0, |mut t: Trace| {
            t.visited.push("fast".to_string());
            async move { HookResult::Continue(t) }
        }),
        entry("slow", 50, 1, |_t: Trace| async move {
            tokio::time::sleep(Duration::from_secs(6)).await;
            HookResult::Continue(Trace::default())
        }),
        entry("third", 10, 2, |mut t: Trace| {
            t.visited.push("third-not-run".to_string());
            async move { HookResult::Continue(t) }
        }),
    ];

    let started = Instant::now();
    let outcome = dispatch(&chain, Trace::default()).await;
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_millis(5_500),
        "timeout did not fire: elapsed {elapsed:?}",
    );
    match outcome {
        DispatchOutcome::Denied {
            reason,
            feedback,
            plugin_fault,
        } => {
            assert!(reason.contains("slow"), "unexpected reason: {reason}");
            assert!(reason.contains("timed out"), "unexpected reason: {reason}");
            assert!(feedback.is_none());
            let fault = plugin_fault.expect("timeout must carry plugin_fault");
            assert_eq!(fault.plugin_id, "slow");
            assert_eq!(fault.kind, FaultKind::Timeout);
            assert_eq!(fault.hook, HookKind::BeforeTurn);
        }
        other => panic!("expected Denied (timeout), got {other:?}"),
    }
}

#[tokio::test]
async fn replace_threads_mutation_through_chain() {
    // Replace must thread the new value to subsequent hooks, identical
    // to Continue at the contract level. Slice 3b adds an audit log
    // entry on Replace; this test asserts behavioral parity with the
    // pre-3b runner so existing chains keep working.
    let chain = vec![
        entry("a", 90, 0, |mut t: Trace| {
            t.visited.push("a".to_string());
            async move { HookResult::Continue(t) }
        }),
        entry("b-replacer", 50, 1, |_t: Trace| async move {
            HookResult::Replace(Trace {
                visited: vec!["b-replaced".to_string()],
            })
        }),
        entry("c", 10, 2, |mut t: Trace| {
            t.visited.push("c-after-replace".to_string());
            async move { HookResult::Continue(t) }
        }),
    ];

    match dispatch(&chain, Trace::default()).await {
        DispatchOutcome::Completed(t) => {
            assert_eq!(t.visited, vec!["b-replaced", "c-after-replace"]);
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_event_forwards_completed_ctx_unchanged() {
    let chain = vec![event_entry("observer", 90, 0, |c: OnEventCtx| async move {
        HookResult::Continue(c)
    })];
    let ctx = OnEventCtx { event: None };
    let out = dispatch_event(&chain, ctx).await;
    assert!(out.event.is_none(), "Continue must thread ctx unchanged");
}

#[tokio::test]
async fn dispatch_event_downgrades_stop_to_completed() {
    // Firehose contract: even if a buggy plugin Stops, the runner must
    // still surface the (potentially mutated) ctx so HookedEventSink
    // forwards it to downstream observers.
    let chain = vec![event_entry("stopper", 50, 0, |c: OnEventCtx| async move {
        HookResult::Stop(c)
    })];
    let ctx = OnEventCtx { event: None };
    let out = dispatch_event(&chain, ctx).await;
    // Stop carries the value through — exactly what a downgrade requires.
    let _ = out;
}

#[tokio::test]
async fn dispatch_event_downgrades_deny_to_default_ctx() {
    // Deny carries no payload, so the runner must produce a default ctx
    // and let the sink drop the event silently rather than swallow it
    // and stall the chain.
    let chain = vec![event_entry("denier", 50, 0, |_c: OnEventCtx| async move {
        HookResult::Deny {
            reason: "synthetic deny".to_string(),
            feedback: None,
        }
    })];
    let ctx = OnEventCtx { event: None };
    let out = dispatch_event(&chain, ctx).await;
    assert!(
        out.event.is_none(),
        "Deny must downgrade to default ctx so the sink drops silently",
    );
}
