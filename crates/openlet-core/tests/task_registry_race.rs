//! `TaskRegistry::admit` quota under concurrent admits +
//! explicit-finalize coverage + a low-level panic-safety demonstration.
//!
//! The registry uses `AtomicUsize::fetch_add(AcqRel)` to claim a quota
//! slot. With quota = 4 and 8 concurrent admits, exactly 4 must
//! succeed and 4 must fail with `SubagentQuotaExceeded`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use openlet_core::runtime::subagent::{SpawnError, TaskHandle, TaskId, TaskRegistry, TaskStatus};
use openlet_core::types::session::SessionId;
use rust_decimal::Decimal;
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;

fn make_handle(root: SessionId) -> TaskHandle {
    TaskHandle {
        status: Arc::new(RwLock::new(TaskStatus::Running)),
        output: Arc::new(RwLock::new(String::new())),
        cost_usd: Arc::new(RwLock::new(Decimal::ZERO)),
        cancel: CancellationToken::new(),
        finished: Arc::new(Notify::new()),
        root_session_id: root,
        parent_session_id: root,
        delivery: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        settled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        inbox_notify: Arc::new(Notify::new()),
        inbox: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admit_quota_exact_under_concurrent_pressure() {
    const QUOTA: usize = 4;
    const N: usize = 8;
    const ITERS: usize = 100;

    let registry = Arc::new(TaskRegistry::new(QUOTA));
    let root = SessionId::new();

    for _ in 0..ITERS {
        let admitted = Arc::new(AtomicUsize::new(0));
        let denied = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let registry = Arc::clone(&registry);
                let admitted = Arc::clone(&admitted);
                let denied = Arc::clone(&denied);
                tokio::spawn(async move {
                    match registry.admit(root) {
                        Ok(id) => {
                            admitted.fetch_add(1, Ordering::SeqCst);
                            id
                        }
                        Err(SpawnError::SubagentQuotaExceeded { .. }) => {
                            denied.fetch_add(1, Ordering::SeqCst);
                            TaskId::new()
                        }
                        Err(other) => panic!("unexpected admit error: {other:?}"),
                    }
                })
            })
            .collect();

        let ids: Vec<TaskId> = futures::future::join_all(handles)
            .await
            .into_iter()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(
            admitted.load(Ordering::SeqCst),
            QUOTA,
            "exactly QUOTA admits must succeed"
        );
        assert_eq!(
            denied.load(Ordering::SeqCst),
            N - QUOTA,
            "remainder must hit SubagentQuotaExceeded"
        );

        // Drain the slots. Only the IDs from successful admits hold a
        // counter — denied admits already rolled back via fetch_sub
        // inside `admit`. We don't know which IDs succeeded, so we
        // install a placeholder handle for each and call finalize once
        // per slot.
        for id in ids.iter().take(QUOTA) {
            registry.insert(*id, make_handle(root));
        }
        for id in ids.iter().take(QUOTA) {
            registry.finalize(*id);
        }

        // Counter back to zero — next iteration starts fresh.
        let next = registry.admit(root).unwrap();
        registry.insert(next, make_handle(root));
        registry.finalize(next);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_finalize_decrements_counter_on_success_path() {
    // Mirrors the success path at subagent_spawner.rs:132,177 — admit,
    // install handle, finalize. Counter must return to zero.
    let registry = Arc::new(TaskRegistry::new(2));
    let root = SessionId::new();

    let id = registry.admit(root).expect("first admit ok");
    registry.insert(id, make_handle(root));
    registry.finalize(id);

    // After finalize, slot is free again — admitting twice in a row
    // must succeed up to the cap.
    for _ in 0..3 {
        let id = registry.admit(root).unwrap();
        registry.insert(id, make_handle(root));
        registry.finalize(id);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explicit_finalize_decrements_counter_on_error_path() {
    // Mirrors the error path at subagent_spawner.rs:350 — admit
    // succeeded, but the spawn failed somewhere downstream. The
    // spawner still calls finalize so the counter doesn't leak.
    let registry = Arc::new(TaskRegistry::new(1));
    let root = SessionId::new();

    let id = registry.admit(root).unwrap();
    registry.insert(id, make_handle(root));

    // Quota of 1 — second admit must fail.
    assert!(matches!(
        registry.admit(root),
        Err(SpawnError::SubagentQuotaExceeded { .. })
    ));

    // Simulated error: caller calls finalize anyway.
    registry.finalize(id);

    // Slot recovered.
    let id2 = registry.admit(root).unwrap();
    registry.insert(id2, make_handle(root));
    registry.finalize(id2);
}

#[tokio::test]
async fn release_quota_rolls_back_admit_when_handle_install_skipped() {
    // Verifies the `release_quota` API used when admit succeeds but
    // installation fails (e.g. unknown subagent slug).
    let registry = TaskRegistry::new(1);
    let root = SessionId::new();

    let _id = registry.admit(root).unwrap();
    // Don't install. Roll back via release_quota.
    registry.release_quota(root);

    // Quota recovered.
    let id2 = registry.admit(root).unwrap();
    registry.insert(id2, make_handle(root));
    registry.finalize(id2);
}

/// `await_completion` must NOT return "task vanished" (`None`) when the
/// driver finalizes (removes the registry entry + releases quota) immediately
/// after flipping status to terminal. The fix reads the snapshot from the
/// already-cloned handle instead of re-looking-up the (now-removed) entry.
///
/// We also assert quota is released so a subsequent spawn on the same root
/// succeeds — proving `finalize` was left intact (the red-team correction).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn await_completion_returns_output_when_finalize_races_set_status() {
    const ITERS: usize = 200;

    for _ in 0..ITERS {
        let registry = Arc::new(TaskRegistry::new(1));
        let root = SessionId::new();

        let id = registry.admit(root).expect("admit ok");
        registry.insert(id, make_handle(root));

        // Seed some output so a successful completion is observable.
        registry.append_output(id, "subagent result").await;

        let waiter = {
            let registry = Arc::clone(&registry);
            tokio::spawn(async move { registry.await_completion(id).await })
        };

        // Race: flip to terminal, yield, then finalize (removes the entry +
        // releases quota) — exactly the driver's success-path ordering.
        let driver = {
            let registry = Arc::clone(&registry);
            tokio::spawn(async move {
                registry.set_status(id, TaskStatus::Finished).await;
                tokio::task::yield_now().await;
                registry.finalize(id);
            })
        };

        driver.await.unwrap();
        let snapshot = waiter.await.unwrap();

        let snapshot =
            snapshot.expect("await_completion must return the completed task, not 'vanished'");
        assert_eq!(snapshot.status, TaskStatus::Finished);
        assert_eq!(snapshot.output, "subagent result");
        assert!(snapshot.finished);

        // Quota released by finalize — a fresh spawn on the same root succeeds.
        let id2 = registry
            .admit(root)
            .expect("quota released; second admit ok");
        registry.insert(id2, make_handle(root));
        registry.finalize(id2);
    }
}

/// The raw registry deliberately has explicit ownership: a caller that admits
/// and then abandons a task without installing/finalizing its handle leaks its
/// own quota. Production `RuntimeSubagentSpawner` wraps the entire driver in
/// `catch_unwind` and finalizes the task on every exceptional exit; this
/// ignored demonstration protects that API boundary from being confused with
/// an automatic RAII guard.
#[tokio::test]
#[ignore = "documents raw TaskRegistry ownership; RuntimeSubagentSpawner catches driver panics"]
async fn panic_between_admit_and_finalize_leaks_quota_slot() {
    let registry = Arc::new(TaskRegistry::new(1));
    let root = SessionId::new();

    // Driver task: admit, install handle, then panic BEFORE finalize.
    let registry_clone = Arc::clone(&registry);
    let panicked = tokio::spawn(async move {
        let id = registry_clone.admit(root).expect("admit succeeds");
        registry_clone.insert(id, make_handle(root));
        panic!("simulated panic before finalize");
    })
    .await;
    assert!(panicked.is_err(), "spawned task panicked as expected");

    // Slot leaked: a second admit must FAIL because the counter
    // wasn't decremented. When the production fix lands, this admit
    // will succeed and the assert below will start failing — at
    // which point the test should be un-ignored and the leak text
    // removed.
    let result = registry.admit(root);
    assert!(
        matches!(result, Err(SpawnError::SubagentQuotaExceeded { .. })),
        "quota slot leaked as documented (current behaviour); \
         when this assertion stops holding, production has been fixed"
    );
}
