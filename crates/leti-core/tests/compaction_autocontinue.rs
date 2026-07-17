//! Dispatch-level test for the OnCompaction `autocontinue` toggle.
//!
//! Scope: verifies that a plugin returning `Replace` with
//! `autocontinue = false` from the After-phase chain successfully
//! threads the field through `dispatch` so the runtime can read it.
//!
//! Why dispatch-level rather than full integration through `run_loop`:
//! the existing test infrastructure for `run_loop` would require staging
//! a provider mock that streams enough tokens to trigger compaction, a
//! memory store with the post-compaction projection wired up, and a
//! cancellation path. That scaffolding is heavier than this single-slice
//! change warrants. The runtime branch that reads `autocontinue` is a
//! straightforward `if let DispatchOutcome::Completed(ref ctx) = ...`
//! check colocated with the dispatch call, so a dispatch-level test
//! covers the mechanical contract; the runtime wiring is verified by
//! `cargo build` + manual review of `turn_loop.rs`.

use std::sync::Arc;

use leti_core::dispatch::{DispatchOutcome, HookChains, HookEntry, dispatch};
use leti_core::hooks::{
    HookKind, HookResult, Priority,
    io::{CompactionPhase, OnCompactionCtx},
};

#[tokio::test]
async fn autocontinue_defaults_true() {
    let ctx = OnCompactionCtx::default();
    assert!(
        ctx.autocontinue,
        "default autocontinue must be true so existing plugins are unchanged"
    );
}

#[tokio::test]
async fn after_phase_replace_with_autocontinue_false_threads_through_dispatch() {
    let mut chains = HookChains::new();
    chains.on_compaction.push(HookEntry::<OnCompactionCtx> {
        manifest_id: "pause-after-compact".into(),
        priority: Priority(50),
        registration_index: 0,
        kind: HookKind::OnCompaction,
        func: Arc::new(|mut c: OnCompactionCtx| {
            Box::pin(async move {
                // Only flip on the After phase — plugins should leave
                // the Before-phase ctx alone for this toggle.
                if matches!(c.phase, CompactionPhase::After) {
                    c.autocontinue = false;
                }
                HookResult::Replace(c)
            })
        }),
    });

    let after_ctx = OnCompactionCtx {
        session_id: None,
        phase: CompactionPhase::After,
        message_count: 7,
        autocontinue: true,
    };
    let outcome = dispatch(&chains.on_compaction, after_ctx).await;
    match outcome {
        DispatchOutcome::Completed(c) => {
            assert!(matches!(c.phase, CompactionPhase::After));
            assert_eq!(c.message_count, 7);
            assert!(
                !c.autocontinue,
                "plugin returned autocontinue=false; runtime should observe the flipped value"
            );
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[tokio::test]
async fn before_phase_unchanged_when_plugin_only_targets_after() {
    let mut chains = HookChains::new();
    chains.on_compaction.push(HookEntry::<OnCompactionCtx> {
        manifest_id: "pause-after-compact".into(),
        priority: Priority(50),
        registration_index: 0,
        kind: HookKind::OnCompaction,
        func: Arc::new(|mut c: OnCompactionCtx| {
            Box::pin(async move {
                if matches!(c.phase, CompactionPhase::After) {
                    c.autocontinue = false;
                }
                HookResult::Replace(c)
            })
        }),
    });

    let before_ctx = OnCompactionCtx {
        session_id: None,
        phase: CompactionPhase::Before,
        message_count: 3,
        autocontinue: true,
    };
    let outcome = dispatch(&chains.on_compaction, before_ctx).await;
    match outcome {
        DispatchOutcome::Completed(c) => {
            assert!(matches!(c.phase, CompactionPhase::Before));
            assert!(
                c.autocontinue,
                "Before-phase ctx must not be mutated by an After-targeting plugin"
            );
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[tokio::test]
async fn no_plugins_keeps_default_autocontinue() {
    // O(1) skip path: an empty chain still threads the input through,
    // preserving the runtime's default `autocontinue = true`.
    let chains = HookChains::new();
    let after_ctx = OnCompactionCtx {
        session_id: None,
        phase: CompactionPhase::After,
        message_count: 0,
        autocontinue: true,
    };
    let outcome = dispatch(&chains.on_compaction, after_ctx).await;
    match outcome {
        DispatchOutcome::Completed(c) => assert!(c.autocontinue),
        other => panic!("expected Completed, got {other:?}"),
    }
}
