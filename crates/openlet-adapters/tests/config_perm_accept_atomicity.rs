//! `ConfigPermissionMgr::accept_ask` atomicity.
//!
//! Two invariants under test:
//!
//! 1. **Restore on persist failure**: when the underlying repo's
//!    `record` errors, `accept_ask` re-inserts the pending entry and
//!    returns the error. `pending_count()` MUST equal the value before
//!    the failed accept.
//!
//! 2. **Double-accept race**: two concurrent `accept_ask` calls on the
//!    same `ask_id` — exactly one Ok, the other `AskExpired`.
//!
//! The repo type is concrete (`SqlitePermissionRepo`); to force a
//! persist failure we close the underlying pool before calling
//! `accept_ask`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use openlet_adapters::config_perm::ConfigPermissionMgr;
use openlet_adapters::sqlite::SqliteMemoryStore;
use openlet_adapters::sqlite::open_in_memory;
use openlet_adapters::sqlite::permission_repo::SqlitePermissionRepo;
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::error::PermissionError;
use openlet_core::types::agent::AgentId;
use openlet_core::types::permission::{
    AlwaysScope, Decision, PermissionAction, PermissionCtx, PermissionMode, PermissionRequest,
};
use openlet_core::types::session::SessionId;
use sqlx::SqlitePool;

/// Seed a real session row so `permission_decisions.session_id` FK
/// passes.
async fn make_session(pool: &SqlitePool) -> SessionId {
    let store = SqliteMemoryStore::new(pool.clone());
    store
        .create_session(AgentId::new(), None)
        .await
        .expect("create_session")
}

/// Small helper that submits an ask via `check()` against a manager
/// whose ruleset doesn't contain an explicit decision. With mode =
/// `ReadOnly`, the fallback action is `Ask`, which inserts a pending
/// entry and returns `Pending { ask_id }`.
async fn submit_ask(
    mgr: &ConfigPermissionMgr,
    session: SessionId,
) -> openlet_core::types::permission::AskId {
    let ctx = PermissionCtx {
        session_id: session,
        mode: PermissionMode::ReadOnly,
    };
    let req = PermissionRequest {
        permission: "test:perm".into(),
        reason: None,
        timeout: None,
    };
    match mgr.check(ctx, req).await.unwrap() {
        Decision::Pending { ask_id } => ask_id,
        other => panic!("expected Pending; got {other:?}"),
    }
}

#[tokio::test]
async fn accept_ask_restores_pending_on_persist_failure() {
    // Pool will be closed before the accept_ask call to force the
    // INSERT inside SqlitePermissionRepo::record to error.
    let pool = open_in_memory().await.unwrap();
    let session = make_session(&pool).await;
    let repo = SqlitePermissionRepo::new(pool.clone());
    let mgr = ConfigPermissionMgr::new().with_repo(repo);

    let ask_id = submit_ask(&mgr, session).await;
    assert_eq!(mgr.pending_count(), 1);

    // Close the pool so any subsequent record() fails with an Io error.
    pool.close().await;

    let scope = AlwaysScope::Session { id: session };
    let err = mgr
        .accept_ask(ask_id, scope, PermissionAction::Allow)
        .await
        .expect_err("accept_ask must fail when persistence fails");

    // The pending entry MUST be restored — without restoration we'd
    // leak an Allow when the persistence layer transient-fails.
    assert_eq!(
        mgr.pending_count(),
        1,
        "pending entry restored after persist failure (count expected 1)"
    );
    assert!(
        matches!(err, PermissionError::Io(_)),
        "expected Io error after pool close; got {err:?}"
    );
}

/// Verify-only — crash AFTER `repo.record` succeeds but BEFORE the
/// in-memory `inner.push`. The persisted rule is orphaned in-memory for the
/// remainder of that process, but the durability contract says it MUST be
/// recovered on the next boot via the load path (`hydrate`). This test
/// simulates the crash by persisting a rule directly through the repo
/// (skipping the in-memory push entirely), then constructs a FRESH manager
/// over the same pool and hydrates it — the rule must be active afterwards.
///
/// This documents and guards the recovery guarantee. It also justifies why
/// `accept_ask` keeps the safe persist-first-then-push order (reversing it
/// would open a TOCTOU privilege-escalation window): persist is the source of
/// truth, the in-memory push is a same-process cache that the load path
/// rebuilds.
#[tokio::test]
async fn persisted_rule_recovered_on_reload_after_crash_before_inmemory_push() {
    let pool = open_in_memory().await.unwrap();
    let session = make_session(&pool).await;

    // Simulate the crash-after-persist orphan: write the rule straight to the
    // repo and DO NOT push it into any in-memory ruleset. This is the exact
    // state a process would be in if it died between `repo.record` (manager.rs
    // :294) and `inner.push` (:299).
    let repo = SqlitePermissionRepo::new(pool.clone());
    let rec = openlet_adapters::sqlite::permission_repo::PermissionRecord {
        session_id: session,
        ask_id: openlet_core::types::permission::AskId::new(),
        permission: "test:perm".into(),
        decision: openlet_adapters::sqlite::permission_repo::PersistedDecision::Always,
    };
    repo.record(&rec).await.expect("persist rule");

    // New boot: a fresh manager over the same pool. Before hydrate the rule is
    // NOT yet in memory — a check would fall back to Ask.
    let mgr = ConfigPermissionMgr::new().with_repo(SqlitePermissionRepo::new(pool.clone()));
    let ctx = PermissionCtx {
        session_id: session,
        mode: PermissionMode::ReadOnly,
    };
    let req = PermissionRequest {
        permission: "test:perm".into(),
        reason: None,
        timeout: None,
    };
    match mgr.check(ctx.clone(), req.clone()).await.unwrap() {
        Decision::Pending { .. } => {} // expected pre-hydrate
        other => panic!("expected Pending before hydrate; got {other:?}"),
    }

    // Load path replays persisted Always rules.
    mgr.hydrate(&[session]).await.expect("hydrate");

    // The orphaned-but-persisted rule is now active — recovery guaranteed.
    match mgr.check(ctx, req).await.unwrap() {
        Decision::Allow => {}
        other => panic!("expected Allow after hydrate recovery; got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn double_accept_ask_has_exactly_one_winner() {
    const ITERS: usize = 50;

    for _ in 0..ITERS {
        let pool = open_in_memory().await.unwrap();
        let session = make_session(&pool).await;
        let repo = SqlitePermissionRepo::new(pool);
        let mgr = Arc::new(ConfigPermissionMgr::new().with_repo(repo));

        let ask_id = submit_ask(&mgr, session).await;
        assert_eq!(mgr.pending_count(), 1);

        let oks = Arc::new(AtomicUsize::new(0));
        let expired = Arc::new(AtomicUsize::new(0));

        let m1 = Arc::clone(&mgr);
        let m2 = Arc::clone(&mgr);
        let oks1 = Arc::clone(&oks);
        let oks2 = Arc::clone(&oks);
        let exp1 = Arc::clone(&expired);
        let exp2 = Arc::clone(&expired);

        let h1 = tokio::spawn(async move {
            match m1
                .accept_ask(
                    ask_id,
                    AlwaysScope::Session { id: session },
                    PermissionAction::Allow,
                )
                .await
            {
                Ok(()) => {
                    oks1.fetch_add(1, Ordering::SeqCst);
                }
                Err(PermissionError::AskExpired) => {
                    exp1.fetch_add(1, Ordering::SeqCst);
                }
                Err(other) => panic!("unexpected error: {other:?}"),
            }
        });
        let h2 = tokio::spawn(async move {
            match m2
                .accept_ask(
                    ask_id,
                    AlwaysScope::Session { id: session },
                    PermissionAction::Allow,
                )
                .await
            {
                Ok(()) => {
                    oks2.fetch_add(1, Ordering::SeqCst);
                }
                Err(PermissionError::AskExpired) => {
                    exp2.fetch_add(1, Ordering::SeqCst);
                }
                Err(other) => panic!("unexpected error: {other:?}"),
            }
        });
        h1.await.unwrap();
        h2.await.unwrap();

        assert_eq!(oks.load(Ordering::SeqCst), 1, "exactly one accept wins");
        assert_eq!(
            expired.load(Ordering::SeqCst),
            1,
            "the loser must report AskExpired"
        );
        assert_eq!(mgr.pending_count(), 0);
    }
}
