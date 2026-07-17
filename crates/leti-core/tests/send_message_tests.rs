//! Phase 4 inter-agent messaging — roster + mailbox registry semantics.
//!
//! Covers the security-critical contracts:
//!   - unique handle names for same-slug siblings (Finding 10);
//!   - liveness: a finalized sibling is no longer addressable (Finding 2);
//!   - gen-check data (a rebound name gets a fresh generation);
//!   - mailbox bounds: over-length body + inbox depth cap (Finding 2);
//!   - roster snapshot ordering for the SSE frame.
//!
//! The `send_message` tool's privilege + hierarchy checks are exercised at
//! the server E2E layer (they need a session store); here we pin the
//! registry primitives the tool builds on.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use leti_core::runtime::subagent::{
    HandleName, SpawnError, TaskHandle, TaskId, TaskRegistry, TaskStatus,
};
use leti_core::types::session::SessionId;
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
        settled: Arc::new(AtomicBool::new(false)),
        inbox_notify: Arc::new(Notify::new()),
        inbox: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
    }
}

/// Admit + install a live task under `root`, returning its id.
fn spawn_live(reg: &TaskRegistry, root: SessionId) -> TaskId {
    let id = reg.admit(root).expect("admit");
    reg.insert(id, make_handle(root));
    id
}

// ---- unique names ----------------------------------------------------

#[test]
fn same_slug_siblings_get_unique_handle_names() {
    let reg = TaskRegistry::new(8);
    let root = SessionId::new();
    let parent = SessionId::new();
    let a = spawn_live(&reg, root);
    let b = spawn_live(&reg, root);

    let (name_a, _) = reg.register_name(root, "reviewer", a, parent, Arc::from(vec![]));
    let (name_b, _) = reg.register_name(root, "reviewer", b, parent, Arc::from(vec![]));

    assert_eq!(
        name_a,
        HandleName("reviewer".into()),
        "first wins bare slug"
    );
    assert_eq!(
        name_b,
        HandleName("reviewer#2".into()),
        "same-slug sibling auto-suffixed"
    );
    // Both are individually addressable.
    assert_eq!(reg.resolve_name(root, &name_a).unwrap().task_id, a);
    assert_eq!(reg.resolve_name(root, &name_b).unwrap().task_id, b);
}

// ---- liveness --------------------------------------------------------

#[test]
fn removed_sibling_is_not_addressable() {
    let reg = TaskRegistry::new(8);
    let root = SessionId::new();
    let parent = SessionId::new();
    let a = spawn_live(&reg, root);
    let (name, _) = reg.register_name(root, "worker", a, parent, Arc::from(vec![]));

    assert!(
        reg.resolve_name(root, &name).is_some(),
        "addressable while live"
    );
    reg.remove_from_roster(root, &name);
    assert!(
        reg.resolve_name(root, &name).is_none(),
        "removed sibling must not resolve (no silent misroute)"
    );
}

// ---- gen-check -------------------------------------------------------

#[test]
fn rebound_name_gets_fresh_generation() {
    let reg = TaskRegistry::new(8);
    let root = SessionId::new();
    let parent = SessionId::new();
    let a = spawn_live(&reg, root);
    let (name_a, gen_a) = reg.register_name(root, "solo", a, parent, Arc::from(vec![]));
    reg.remove_from_roster(root, &name_a);

    // A new task rebinds the SAME bare name (the first is gone).
    let b = spawn_live(&reg, root);
    let (name_b, gen_b) = reg.register_name(root, "solo", b, parent, Arc::from(vec![]));

    assert_eq!(name_a, name_b, "bare name reused after removal");
    assert!(
        gen_b > gen_a,
        "rebind must bump generation ({gen_b} > {gen_a}) so a stale send is caught"
    );
    assert_eq!(reg.resolve_name(root, &name_b).unwrap().generation, gen_b);
}

// ---- mailbox bounds --------------------------------------------------

#[test]
fn push_message_rejects_over_length_body() {
    let reg = TaskRegistry::with_message_limits(8, 4, 16);
    let root = SessionId::new();
    let id = spawn_live(&reg, root);

    let ok = reg.push_message(id, "peer", "short");
    assert!(ok.is_ok(), "within cap accepted");

    let too_long = "x".repeat(17);
    assert!(
        matches!(
            reg.push_message(id, "peer", &too_long),
            Err(SpawnError::MessageRejected(_))
        ),
        "over-length body rejected (bound length, not just depth)"
    );
}

#[test]
fn push_message_enforces_inbox_depth_cap() {
    let reg = TaskRegistry::with_message_limits(8, 2, 4096);
    let root = SessionId::new();
    let id = spawn_live(&reg, root);

    assert!(reg.push_message(id, "p", "one").is_ok());
    assert!(reg.push_message(id, "p", "two").is_ok());
    // Third exceeds depth cap of 2.
    assert!(
        matches!(
            reg.push_message(id, "p", "three"),
            Err(SpawnError::MessageRejected(_))
        ),
        "inbox depth cap enforced"
    );

    // Draining frees capacity again.
    let drained = reg.drain_inbox(id);
    assert_eq!(drained.len(), 2);
    assert_eq!(drained[0].body, "one", "FIFO drain order");
    assert!(reg.push_message(id, "p", "after-drain").is_ok());
}

#[test]
fn push_message_to_finalized_task_is_typed_error() {
    let reg = TaskRegistry::new(8);
    let root = SessionId::new();
    let id = spawn_live(&reg, root);
    reg.finalize(id);

    assert!(
        matches!(
            reg.push_message(id, "p", "hello"),
            Err(SpawnError::MessageRejected(_))
        ),
        "message to a finalized task is a typed error, never a silent drop"
    );
}

// ---- roster snapshot -------------------------------------------------

#[test]
fn roster_snapshot_is_name_sorted() {
    let reg = TaskRegistry::new(8);
    let root = SessionId::new();
    let parent = SessionId::new();
    let a = spawn_live(&reg, root);
    let b = spawn_live(&reg, root);
    reg.register_name(root, "zeta", a, parent, Arc::from(vec![]));
    reg.register_name(root, "alpha", b, parent, Arc::from(vec![]));

    let snap = reg.roster_snapshot(root);
    let names: Vec<String> = snap.iter().map(|(n, _, _)| n.0.clone()).collect();
    assert_eq!(names, vec!["alpha", "zeta"], "snapshot sorted by name");
}
