//! `promote_task` tool + registry promotion/lifetime-budget coverage.
//!
//! Phase 3: promotion marks an already-background task for
//! auto-notification (`was_promoted`); the driver reads it at settle to
//! route output through the parent injector. Also covers the per-root
//! cumulative lifetime spawn budget that fail-closes a runaway
//! injection-driven spawn loop.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use openlet_core::runtime::subagent::{SpawnError, TaskHandle, TaskId, TaskRegistry, TaskStatus};
use openlet_core::tools::Tool;
use openlet_core::tools::builtins::promote_task::{PromoteTaskInput, PromoteTaskTool};
use openlet_core::types::session::SessionId;
use rust_decimal::Decimal;
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;

mod common;
use common::tool_ctx::minimal_tool_ctx_with_registry;

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

// ---- registry promotion ----------------------------------------------

#[tokio::test]
async fn mark_promoted_sets_flag_once_present() {
    let reg = TaskRegistry::new(4);
    let root = SessionId::new();
    let id = reg.admit(root).unwrap();
    reg.insert(id, make_handle(root));

    assert!(!reg.is_promoted(id), "not promoted initially");
    assert!(reg.mark_promoted(id), "mark returns true for live task");
    assert!(reg.is_promoted(id), "flag observable after mark");
}

#[tokio::test]
async fn mark_promoted_unknown_task_is_false_noop() {
    let reg = TaskRegistry::new(4);
    // Never admitted / installed.
    assert!(!reg.mark_promoted(TaskId::new()));
}

// ---- promote_task tool ------------------------------------------------

#[tokio::test]
async fn promote_task_marks_background_task() {
    let reg = Arc::new(TaskRegistry::new(4));
    let root = SessionId::new();
    let id = reg.admit(root).unwrap();
    reg.insert(id, make_handle(root));

    let tool = PromoteTaskTool::new(reg.clone());
    let ctx = minimal_tool_ctx_with_registry(reg.clone());
    let out = tool
        .run(
            ctx,
            PromoteTaskInput {
                task_id: id.0.to_string(),
            },
        )
        .await
        .expect("run ok");

    assert!(out.promoted, "live background task promoted");
    assert!(reg.is_promoted(id), "registry reflects promotion");
}

#[tokio::test]
async fn promote_task_unknown_id_is_ack_not_error() {
    let reg = Arc::new(TaskRegistry::new(4));
    let tool = PromoteTaskTool::new(reg.clone());
    let ctx = minimal_tool_ctx_with_registry(reg.clone());

    // Unknown UUID → promoted=false, still Ok (never an error).
    let out = tool
        .run(
            ctx,
            PromoteTaskInput {
                task_id: TaskId::new().0.to_string(),
            },
        )
        .await
        .expect("run ok");
    assert!(!out.promoted);

    // Invalid (non-UUID) → also a clean ack.
    let ctx2 = minimal_tool_ctx_with_registry(reg.clone());
    let out2 = tool
        .run(
            ctx2,
            PromoteTaskInput {
                task_id: "not-a-uuid".into(),
            },
        )
        .await
        .expect("run ok");
    assert!(!out2.promoted);
}

// ---- lifetime budget --------------------------------------------------

#[tokio::test]
async fn lifetime_budget_fail_closes_after_cap_even_when_slots_free() {
    // Concurrency cap high, lifetime budget low: prove the CUMULATIVE
    // budget rejects further admits even though slots are free (each task
    // finalizes before the next admits — the runaway-injection scenario).
    let reg = TaskRegistry::with_limits(32, 3);
    let root = SessionId::new();

    for _ in 0..3 {
        let id = reg.admit(root).expect("within lifetime budget");
        reg.insert(id, make_handle(root));
        reg.finalize(id); // frees the concurrency slot each time.
    }

    // 4th admit: concurrency slot is free (all finalized), but the
    // cumulative lifetime budget is spent → fail closed.
    assert!(
        matches!(
            reg.admit(root),
            Err(SpawnError::SubagentLifetimeBudgetExceeded { max: 3, .. })
        ),
        "cumulative lifetime budget must fail-close a churn loop"
    );
}

#[tokio::test]
async fn lifetime_budget_is_per_root_not_global() {
    let reg = TaskRegistry::with_limits(32, 2);
    let root_a = SessionId::new();
    let root_b = SessionId::new();

    // Exhaust root A's budget.
    for _ in 0..2 {
        let id = reg.admit(root_a).unwrap();
        reg.insert(id, make_handle(root_a));
        reg.finalize(id);
    }
    assert!(reg.admit(root_a).is_err(), "root A budget spent");

    // Root B is unaffected — budget is per-root.
    let id = reg.admit(root_b).expect("root B has its own budget");
    reg.insert(id, make_handle(root_b));
    reg.finalize(id);
}

#[tokio::test]
async fn concurrency_reject_refunds_lifetime_budget() {
    // A concurrency-rejected admit must NOT consume lifetime budget — only
    // genuine live tasks count against it.
    let reg = TaskRegistry::with_limits(1, 10);
    let root = SessionId::new();

    let id = reg.admit(root).unwrap();
    reg.insert(id, make_handle(root));

    // Concurrency cap = 1: this admit rejects (slot full). It must refund
    // the lifetime increment so the budget isn't silently eroded by
    // rejected attempts.
    assert!(matches!(
        reg.admit(root),
        Err(SpawnError::SubagentQuotaExceeded { .. })
    ));

    reg.finalize(id); // free the slot.

    // Lifetime budget (10) should have only counted the ONE live task, so
    // we can still admit many more.
    for _ in 0..8 {
        let id = reg
            .admit(root)
            .expect("lifetime budget not eroded by rejects");
        reg.insert(id, make_handle(root));
        reg.finalize(id);
    }
}
