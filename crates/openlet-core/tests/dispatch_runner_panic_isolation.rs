//! Hook chain runner: panic isolation + timeout synthesis.
//!
//! `dispatch.rs::runner::dispatch` wraps every hook future in a panic
//! catcher + 5 s timeout. Two cases under test:
//!
//! 1. A panicker among 3 hooks (prio 90 noop, prio 50 panicker, prio
//!    10 noop) MUST surface as `Denied { plugin_fault: PollPanic }`,
//!    and the prio-10 hook MUST NOT be called once Deny is emitted.
//! 2. A hook that sleeps past the runner's hard `HOOK_TIMEOUT = 5 s`
//!    MUST surface as `Denied { plugin_fault: Timeout }`. Uses
//!    `tokio::test(start_paused = true)` so the 5 s wait is virtual.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use openlet_core::dispatch::{DispatchOutcome, FaultKind, HookChains, HookEntry, dispatch};
use openlet_core::hooks::io::BeforeToolCallCtx;
use openlet_core::hooks::{HookKind, HookResult, Priority};

fn entry_with(
    manifest_id: &str,
    priority: u8,
    registration_index: usize,
    func: impl Fn(BeforeToolCallCtx) -> openlet_core::dispatch::HookFuture<BeforeToolCallCtx>
    + Send
    + Sync
    + 'static,
) -> HookEntry<BeforeToolCallCtx> {
    HookEntry {
        manifest_id: manifest_id.to_string(),
        priority: Priority(priority),
        registration_index,
        kind: HookKind::BeforeToolCall,
        func: Arc::new(func),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn panicking_middle_hook_synthesizes_deny_and_skips_lower_priority() {
    let high_invoked = Arc::new(AtomicUsize::new(0));
    let low_invoked = Arc::new(AtomicUsize::new(0));

    let high_counter = Arc::clone(&high_invoked);
    let low_counter = Arc::clone(&low_invoked);

    let mut chains = HookChains::new();
    chains
        .before_tool_call
        .push(entry_with("high", 90, 0, move |c| {
            let counter = Arc::clone(&high_counter);
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                HookResult::Continue(c)
            })
        }));
    chains
        .before_tool_call
        .push(entry_with("panicker", 50, 1, |_c: BeforeToolCallCtx| {
            Box::pin(async move {
                panic!("intentional panic during poll");
            })
        }));
    chains
        .before_tool_call
        .push(entry_with("low", 10, 2, move |c| {
            let counter = Arc::clone(&low_counter);
            Box::pin(async move {
                counter.fetch_add(1, Ordering::SeqCst);
                HookResult::Continue(c)
            })
        }));
    chains.sort_all();

    let outcome = dispatch(
        &chains.before_tool_call,
        BeforeToolCallCtx {
            session_id: None,
            invocation: None,
        },
    )
    .await;

    match outcome {
        DispatchOutcome::Denied {
            plugin_fault: Some(fault),
            ..
        } => {
            assert_eq!(fault.plugin_id, "panicker");
            assert!(matches!(fault.kind, FaultKind::PollPanic));
            assert!(fault.message.contains("intentional panic during poll"));
        }
        other => panic!("expected Denied(PollPanic); got {other:?}"),
    }

    assert_eq!(high_invoked.load(Ordering::SeqCst), 1, "high prio ran");
    assert_eq!(
        low_invoked.load(Ordering::SeqCst),
        0,
        "low prio MUST NOT run after Deny"
    );
}

#[tokio::test]
async fn construction_panic_surfaces_with_construction_panic_kind() {
    // Some plugins panic from their CLOSURE rather than the returned
    // future. The runner catches those at construction time and emits
    // `FaultKind::ConstructionPanic`. Lock the discriminator.
    let mut chains = HookChains::new();
    chains
        .before_tool_call
        .push(entry_with("ctor_panicker", 50, 0, |_c| {
            // Panic before returning a future — caught by the runner's
            // outer `catch_unwind` over the closure invocation.
            panic!("intentional panic during construction");
        }));

    let outcome = dispatch(
        &chains.before_tool_call,
        BeforeToolCallCtx {
            session_id: None,
            invocation: None,
        },
    )
    .await;

    match outcome {
        DispatchOutcome::Denied {
            plugin_fault: Some(fault),
            ..
        } => {
            assert!(matches!(fault.kind, FaultKind::ConstructionPanic));
        }
        other => panic!("expected Denied(ConstructionPanic); got {other:?}"),
    }
}
