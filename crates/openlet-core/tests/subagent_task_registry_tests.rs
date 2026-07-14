//! Unit coverage for `TaskRegistry` — admit/release/finalize/set_status,
//! the await-after-finalize race, output cap, and the one-shot terminal
//! side-effect guard (`claim_settle`).
//!
//! Phase 1 (tests-first): these pin the CONTRACT (quota balances, terminal
//! snapshot survives finalize, side-effect fires once) rather than the
//! internal call sequence, so the Phase 2/3 redesign can refactor freely.

use std::sync::Arc;

use openlet_core::runtime::subagent::{MAX_OUTPUT_BYTES, TaskHandle, TaskRegistry, TaskStatus};
use openlet_core::types::session::SessionId;
use rust_decimal::Decimal;
use std::sync::atomic::AtomicBool;
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
        settled: Arc::new(AtomicBool::new(false)),
        inbox_notify: Arc::new(Notify::new()),
        was_promoted: Arc::new(AtomicBool::new(false)),
        inbox: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
    }
}

// ---- Quota balance ---------------------------------------------------

#[tokio::test]
async fn admit_then_finalize_returns_balance_to_zero() {
    let reg = TaskRegistry::new(4);
    let root = SessionId::new();

    let id = reg.admit(root).expect("admit");
    reg.insert(id, make_handle(root));
    reg.finalize(id);

    // Balance restored: we can admit up to the full cap again.
    for _ in 0..4 {
        let id = reg.admit(root).expect("post-finalize admit");
        reg.insert(id, make_handle(root));
        reg.finalize(id);
    }
}

#[tokio::test]
async fn admit_past_cap_returns_quota_exceeded() {
    use openlet_core::runtime::subagent::SpawnError;
    let reg = TaskRegistry::new(2);
    let root = SessionId::new();

    let a = reg.admit(root).unwrap();
    reg.insert(a, make_handle(root));
    let b = reg.admit(root).unwrap();
    reg.insert(b, make_handle(root));

    assert!(matches!(
        reg.admit(root),
        Err(SpawnError::SubagentQuotaExceeded {
            in_flight: 2,
            max: 2
        })
    ));
}

#[tokio::test]
async fn double_finalize_floors_at_zero_no_wrap() {
    let reg = TaskRegistry::new(2);
    let root = SessionId::new();

    let id = reg.admit(root).unwrap();
    reg.insert(id, make_handle(root));
    reg.finalize(id);
    // Second finalize on an already-removed id is a harmless no-op —
    // `saturating_dec` floors at 0, so the counter does NOT wrap to
    // usize::MAX (which would permanently wedge the quota).
    reg.finalize(id);

    // Cap still honored: we can admit exactly `max` and no more.
    let a = reg.admit(root).unwrap();
    reg.insert(a, make_handle(root));
    let b = reg.admit(root).unwrap();
    reg.insert(b, make_handle(root));
    assert!(
        reg.admit(root).is_err(),
        "counter must not have underflowed"
    );
}

#[tokio::test]
async fn double_release_quota_floors_at_zero() {
    let reg = TaskRegistry::new(1);
    let root = SessionId::new();

    let _id = reg.admit(root).unwrap();
    reg.release_quota(root);
    reg.release_quota(root); // extra release — must floor, not wrap.

    // Fresh admit still succeeds and cap still enforced.
    let a = reg.admit(root).unwrap();
    reg.insert(a, make_handle(root));
    assert!(reg.admit(root).is_err());
}

// ---- Terminal snapshot survives finalize (await race) ----------------

#[tokio::test]
async fn late_await_after_finalize_returns_terminal_snapshot() {
    let reg = TaskRegistry::new(2);
    let root = SessionId::new();

    let id = reg.admit(root).unwrap();
    reg.insert(id, make_handle(root));
    reg.append_output(id, "done").await;
    reg.set_status(id, TaskStatus::Finished).await;
    reg.finalize(id); // removes live entry.

    // Await AFTER finalize must fall back to the terminal cache, not None.
    let snap = reg
        .await_completion(id)
        .await
        .expect("terminal snapshot from cache, not 'vanished'");
    assert_eq!(snap.status, TaskStatus::Finished);
    assert_eq!(snap.output, "done");
    assert!(snap.finished);
}

#[tokio::test]
async fn await_subscribed_before_set_status_wakes() {
    let reg = Arc::new(TaskRegistry::new(2));
    let root = SessionId::new();
    let id = reg.admit(root).unwrap();
    reg.insert(id, make_handle(root));

    let waiter = {
        let reg = reg.clone();
        tokio::spawn(async move { reg.await_completion(id).await })
    };
    // Give the waiter a chance to subscribe, then flip status.
    tokio::task::yield_now().await;
    reg.set_status(id, TaskStatus::Finished).await;

    let snap = tokio::time::timeout(std::time::Duration::from_secs(5), waiter)
        .await
        .expect("no lost-wakeup hang")
        .unwrap()
        .expect("snapshot");
    assert_eq!(snap.status, TaskStatus::Finished);
}

// ---- Output cap ------------------------------------------------------

#[tokio::test]
async fn append_output_past_cap_truncates_and_is_bounded() {
    let reg = TaskRegistry::new(2);
    let root = SessionId::new();
    let id = reg.admit(root).unwrap();
    reg.insert(id, make_handle(root));

    // One append just over the cap trips the sentinel.
    let big = "x".repeat(MAX_OUTPUT_BYTES + 1);
    reg.append_output(id, &big).await;
    let snap = reg.poll_async(id).await.expect("snap");
    assert_eq!(snap.output, "[truncated]");

    // Further appends are a no-op — buffer stays bounded.
    reg.append_output(id, "more").await;
    let snap = reg.poll_async(id).await.expect("snap");
    assert_eq!(snap.output, "[truncated]");
}

// ---- One-shot terminal side-effect guard -----------------------------

#[tokio::test]
async fn claim_settle_fires_exactly_once() {
    let reg = TaskRegistry::new(2);
    let root = SessionId::new();
    let id = reg.admit(root).unwrap();
    reg.insert(id, make_handle(root));

    assert!(reg.claim_settle(id), "first claim wins");
    assert!(!reg.claim_settle(id), "second claim loses");
    assert!(!reg.claim_settle(id), "third claim loses");
}

#[tokio::test]
async fn claim_settle_on_finalized_task_returns_false() {
    let reg = TaskRegistry::new(2);
    let root = SessionId::new();
    let id = reg.admit(root).unwrap();
    reg.insert(id, make_handle(root));
    reg.finalize(id);

    // Handle removed — nothing to claim; the side-effect already fired
    // (or the task never settled). Must not panic, returns false.
    assert!(!reg.claim_settle(id));
}

#[tokio::test]
async fn quota_decrement_idempotent_independent_of_settle_guard() {
    // Finding 13: the `settled` guard must NOT gate the quota decrement.
    // Claiming settle then finalizing N times keeps the counter floored,
    // and the quota is fully released.
    let reg = TaskRegistry::new(1);
    let root = SessionId::new();
    let id = reg.admit(root).unwrap();
    reg.insert(id, make_handle(root));

    assert!(reg.claim_settle(id));
    reg.finalize(id);
    reg.finalize(id); // idempotent decrement, unaffected by claimed settle.

    // Slot fully released.
    let id2 = reg
        .admit(root)
        .expect("quota released despite settle claim");
    reg.insert(id2, make_handle(root));
    reg.finalize(id2);
}
