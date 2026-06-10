//! `QuestionRegistry` resolve / claim / cancel races.
//!
//! Three concurrency paths are exercised:
//!
//! 1. Two `resolve(qid, ...)` calls on the same id — exactly one wins,
//!    the other returns `ResolveError::NotFound`.
//! 2. `try_claim_session_slot` race — N concurrent claims for the same
//!    session_id; exactly one returns `true`.
//! 3. `cancel` racing `resolve` — awaiter never deadlocks. Either the
//!    receiver gets a value or the channel closes; never both, never
//!    neither.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use openlet_core::runtime::question_registry::{
    CancelReason, QuestionId, QuestionRegistry, ResolveError,
};
use openlet_core::types::session::SessionId;
use tokio::sync::oneshot;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_resolves_on_same_qid_have_exactly_one_winner() {
    const ITERS: usize = 200;

    for _ in 0..ITERS {
        let reg = Arc::new(QuestionRegistry::new());
        let qid = QuestionId::new();
        let session = SessionId::new();
        let (tx, mut rx) = oneshot::channel::<Vec<usize>>();
        reg.register(qid, session, tx);

        let r1 = Arc::clone(&reg);
        let r2 = Arc::clone(&reg);
        let h1 = tokio::spawn(async move { r1.resolve(qid, session, vec![0]) });
        let h2 = tokio::spawn(async move { r2.resolve(qid, session, vec![1]) });

        let res1 = h1.await.unwrap();
        let res2 = h2.await.unwrap();

        let oks = [&res1, &res2].iter().filter(|r| r.is_ok()).count();
        let nfs = [&res1, &res2]
            .iter()
            .filter(|r| matches!(r, Err(ResolveError::NotFound)))
            .count();
        assert_eq!(oks, 1, "exactly one resolve must succeed");
        assert_eq!(nfs, 1, "the other must report NotFound");

        // Receiver got exactly one payload (either [0] or [1]).
        let payload = rx.try_recv().expect("payload arrived");
        assert!(payload == vec![0] || payload == vec![1]);
        assert_eq!(reg.pending_len(), 0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn try_claim_session_slot_admits_exactly_one_winner() {
    const ITERS: usize = 100;
    const N: usize = 10;

    for _ in 0..ITERS {
        let reg = Arc::new(QuestionRegistry::new());
        let session = SessionId::new();

        let claimed = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let reg = Arc::clone(&reg);
                let claimed = Arc::clone(&claimed);
                tokio::spawn(async move {
                    if reg.try_claim_session_slot(session) {
                        claimed.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(claimed.load(Ordering::SeqCst), 1);

        // Release; another claim succeeds.
        reg.remove_session_slot(session);
        assert!(reg.try_claim_session_slot(session));
        reg.remove_session_slot(session);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_resolve_race_never_deadlocks() {
    const ITERS: usize = 200;

    for _ in 0..ITERS {
        let reg = Arc::new(QuestionRegistry::new());
        let qid = QuestionId::new();
        let session = SessionId::new();
        let (tx, rx) = oneshot::channel::<Vec<usize>>();
        reg.register(qid, session, tx);

        let r1 = Arc::clone(&reg);
        let r2 = Arc::clone(&reg);
        let h_cancel = tokio::spawn(async move { r1.cancel(qid, CancelReason::Operator) });
        let h_resolve = tokio::spawn(async move { r2.resolve(qid, session, vec![42]) });

        h_cancel.await.unwrap();
        let _ = h_resolve.await.unwrap();

        // Awaiter sees one of:
        // - Ok(vec![42]) when resolve won
        // - Err(_) when cancel won (sender dropped)
        // Both terminate the receiver — never deadlock.
        match rx.await {
            Ok(v) => assert_eq!(v, vec![42]),
            Err(_) => { /* sender dropped; expected if cancel won */ }
        }
        assert_eq!(reg.pending_len(), 0);
    }
}
