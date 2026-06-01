//! Integration tests for `BroadcastBus` durable replay + lag tolerance.
//!
//! Two-tier publish: durable events go through `SqliteEventRepo` first
//! (Last-Event-ID assigned) then broadcast; transient events
//! broadcast-only. Tests below lock:
//!
//! 1. Durable publish writes to repo + broadcasts. `replay_since`
//!    returns events strictly after the cursor.
//! 2. Transient publish broadcasts only — replay returns nothing for it.
//! 3. `BroadcastBus::new()` (no repo) succeeds on durable publish but
//!    `replay_since` returns empty.
//! 4. `replay_since_global` returns events across all sessions in id order.
//! 5. Slow subscriber lags via `RecvError::Lagged(n)` rather than blocking
//!    publishers.
//! 6. Publishing with no subscribers does NOT error (broadcast `Err`
//!    suppression).

mod common;

use chrono::Utc;
use common::sqlite_helper::make_pool;
use openlet_adapters::bus::BroadcastBus;
use openlet_adapters::sqlite::SqliteMemoryStore;
use openlet_adapters::sqlite::event_repo::SqliteEventRepo;
use openlet_core::adapters::EventSink;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::{AgentEvent, EventFilter};
use openlet_core::types::message::MessageId;
use openlet_core::types::session::{SessionId, SessionStatus};
use tokio::sync::broadcast::error::RecvError;

async fn make_bus_with_session() -> (BroadcastBus, SessionId) {
    let pool = make_pool().await;
    let repo = SqliteEventRepo::new(pool.clone());
    let mem = SqliteMemoryStore::new(pool);
    let session = mem.create_session(AgentId::new(), None).await.unwrap();
    let bus = BroadcastBus::with_repo(repo);
    (bus, session)
}

fn session_status_event(sid: SessionId) -> AgentEvent {
    AgentEvent::SessionStatus {
        session_id: sid,
        status: SessionStatus::Running,
        at: Utc::now(),
    }
}

fn part_delta_event(sid: SessionId) -> AgentEvent {
    use openlet_core::types::event::DeltaKind;
    AgentEvent::PartDelta {
        session_id: sid,
        message_id: MessageId::new(),
        part_id: openlet_core::types::part::PartId::new(),
        delta_kind: DeltaKind::Text,
        delta: "x".to_string(),
    }
}

#[tokio::test]
async fn durable_publish_assigns_event_id_and_broadcasts() {
    let (bus, session) = make_bus_with_session().await;
    let mut rx = bus.subscribe(EventFilter::default());

    bus.publish(session_status_event(session), Persistence::Durable)
        .await
        .unwrap();

    let delivered = rx.recv().await.unwrap();
    assert!(
        delivered.event_id.is_some(),
        "durable publish must carry Last-Event-ID, got None"
    );
    assert!(matches!(delivered.event, AgentEvent::SessionStatus { .. }));
}

#[tokio::test]
async fn transient_publish_skips_repo_and_broadcast_event_id_is_none() {
    let (bus, session) = make_bus_with_session().await;
    let mut rx = bus.subscribe(EventFilter::default());

    bus.publish(part_delta_event(session), Persistence::Transient)
        .await
        .unwrap();

    let delivered = rx.recv().await.unwrap();
    assert!(
        delivered.event_id.is_none(),
        "transient publish must NOT have an event_id"
    );

    // replay_since for the same session returns no rows for the
    // transient event we just sent.
    let replay = bus.replay_since(session, 0).await.unwrap();
    assert!(
        replay.is_empty(),
        "transient events must not appear in replay, got {} rows",
        replay.len()
    );
}

#[tokio::test]
async fn replay_since_returns_events_strictly_after_cursor() {
    let (bus, session) = make_bus_with_session().await;

    bus.publish(session_status_event(session), Persistence::Durable)
        .await
        .unwrap();
    bus.publish(session_status_event(session), Persistence::Durable)
        .await
        .unwrap();
    bus.publish(session_status_event(session), Persistence::Durable)
        .await
        .unwrap();

    let all = bus.replay_since(session, 0).await.unwrap();
    assert_eq!(all.len(), 3, "expected 3 durable events, got {}", all.len());

    let cursor = all[0].event_id.unwrap();
    let after_first = bus.replay_since(session, cursor).await.unwrap();
    assert_eq!(
        after_first.len(),
        2,
        "after cursor {} expected 2 events, got {}",
        cursor,
        after_first.len()
    );
}

#[tokio::test]
async fn bus_without_repo_publishes_without_event_id_and_replay_is_empty() {
    let (_pool, session) = {
        let pool = make_pool().await;
        let mem = SqliteMemoryStore::new(pool.clone());
        let s = mem.create_session(AgentId::new(), None).await.unwrap();
        (pool, s)
    };
    let bus = BroadcastBus::new();
    let mut rx = bus.subscribe(EventFilter::default());

    // Durable publish without a repo is allowed — event_id stays None.
    bus.publish(session_status_event(session), Persistence::Durable)
        .await
        .unwrap();

    let delivered = rx.recv().await.unwrap();
    assert!(
        delivered.event_id.is_none(),
        "no-repo bus must not synthesize an event_id"
    );

    // replay_since returns empty without a repo.
    let replay = bus.replay_since(session, 0).await.unwrap();
    assert!(replay.is_empty());
}

#[tokio::test]
async fn replay_since_global_returns_cross_session_events_in_id_order() {
    let pool = make_pool().await;
    let repo = SqliteEventRepo::new(pool.clone());
    let mem = SqliteMemoryStore::new(pool);
    let s_a = mem.create_session(AgentId::new(), None).await.unwrap();
    let s_b = mem.create_session(AgentId::new(), None).await.unwrap();
    let bus = BroadcastBus::with_repo(repo);

    bus.publish(session_status_event(s_a), Persistence::Durable)
        .await
        .unwrap();
    bus.publish(session_status_event(s_b), Persistence::Durable)
        .await
        .unwrap();
    bus.publish(session_status_event(s_a), Persistence::Durable)
        .await
        .unwrap();

    let all = bus.replay_since_global(0).await.unwrap();
    assert_eq!(all.len(), 3);
    let ids: Vec<_> = all.iter().map(|e| e.event_id.unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "global replay must come back in id order");
}

#[tokio::test]
async fn publish_with_no_subscribers_does_not_error() {
    let (bus, session) = make_bus_with_session().await;
    // No subscriber attached. Broadcast `Err` is suppressed; durable
    // path must still write through the repo successfully.
    bus.publish(session_status_event(session), Persistence::Durable)
        .await
        .unwrap();

    // Replay confirms the durable write happened despite no subscribers.
    let replay = bus.replay_since(session, 0).await.unwrap();
    assert_eq!(replay.len(), 1);
}

#[tokio::test]
async fn slow_subscriber_lags_rather_than_blocking_publisher() {
    // BROADCAST_CAPACITY is 1024 — fill a chunk and verify a slow
    // subscriber receives `Lagged(n)` instead of stalling the
    // publisher. We deliberately publish > capacity so the slow
    // receiver overflows.
    let (bus, session) = make_bus_with_session().await;
    let mut slow = bus.subscribe(EventFilter::default());

    // Publish more than capacity transient events so they don't
    // touch SQLite (which would slow the test). Transient publishes
    // are pure broadcasts.
    for _ in 0..2048 {
        bus.publish(part_delta_event(session), Persistence::Transient)
            .await
            .unwrap();
    }

    // Slow subscriber polls now — must surface Lagged on first recv.
    let first = slow.recv().await;
    match first {
        Err(RecvError::Lagged(n)) => assert!(n > 0, "Lagged carries positive count"),
        other => panic!("expected Lagged, got {other:?}"),
    }
    // After a Lagged error, subsequent recv resumes — at least 1 of
    // the buffered events must come through.
    let next = slow.recv().await;
    assert!(
        next.is_ok(),
        "after Lagged, recv must yield buffered events, got {next:?}"
    );
}

/// H1 — the monotonic event-id counter MUST be seeded from the persisted
/// `MAX(id)` on first durable publish, NOT from 0. The `events` table is
/// durable across restarts; a counter that restarts at 0 each boot would
/// re-issue ids 1.. and collide with surviving rows on the explicit-PK
/// insert (`append_with_id`), so every durable publish after a restart
/// would fail with a `UNIQUE` violation.
///
/// We simulate a restart by sharing one pool (the durable `events` table)
/// across two `BroadcastBus` instances: the first writes some events, the
/// second is a "fresh boot" over the same data.
#[tokio::test]
async fn durable_publish_after_restart_seeds_counter_from_max_id() {
    let pool = make_pool().await;
    let mem = SqliteMemoryStore::new(pool.clone());
    let session = mem.create_session(AgentId::new(), None).await.unwrap();

    // Boot #1: publish 3 durable events → ids 1, 2, 3.
    {
        let bus = BroadcastBus::with_repo(SqliteEventRepo::new(pool.clone()));
        for _ in 0..3 {
            bus.publish(session_status_event(session), Persistence::Durable)
                .await
                .unwrap();
        }
        let all = bus.replay_since(session, 0).await.unwrap();
        let ids: Vec<_> = all.iter().map(|e| e.event_id.unwrap()).collect();
        assert_eq!(ids, vec![1, 2, 3], "boot #1 assigns 1..=3");
    }

    // Boot #2: brand-new bus over the SAME durable table. Its counter is
    // unseeded; the first durable publish must seed from MAX(id)=3 and
    // assign 4 — NOT 1 (which would be a PK collision).
    {
        let bus = BroadcastBus::with_repo(SqliteEventRepo::new(pool.clone()));
        bus.publish(session_status_event(session), Persistence::Durable)
            .await
            .expect("post-restart durable publish must not collide on PK");

        let all = bus.replay_since(session, 0).await.unwrap();
        let ids: Vec<_> = all.iter().map(|e| e.event_id.unwrap()).collect();
        assert_eq!(
            ids,
            vec![1, 2, 3, 4],
            "post-restart publish continues the id sequence at 4"
        );
    }
}

/// H1 — under concurrent durable publishes, event_ids must be assigned,
/// persisted, AND broadcast in the same strictly-increasing order so a
/// `Last-Event-ID`-tracking SSE subscriber never observes a gap. This
/// guards the replay-seam contract: arrival order on the live channel must
/// match id order (`routes/event.rs` drops live frames `id <= high_water`
/// and replays strictly `id > after`, so an out-of-order broadcast is lost
/// both live AND on replay).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_durable_publishes_broadcast_in_event_id_order() {
    use std::sync::Arc;

    const N: usize = 64;

    let pool = make_pool().await;
    let mem = SqliteMemoryStore::new(pool.clone());
    let session = mem.create_session(AgentId::new(), None).await.unwrap();
    let bus = Arc::new(BroadcastBus::with_repo(SqliteEventRepo::new(pool.clone())));

    // Subscribe BEFORE publishing so every frame is observed live.
    let mut rx = bus.subscribe(EventFilter::default());

    // Fire N concurrent durable publishes.
    let mut handles = Vec::new();
    for _ in 0..N {
        let bus = Arc::clone(&bus);
        handles.push(tokio::spawn(async move {
            bus.publish(session_status_event(session), Persistence::Durable)
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    // Collect the live-broadcast ids in ARRIVAL order.
    let mut live_ids = Vec::with_capacity(N);
    for _ in 0..N {
        let d = rx.recv().await.unwrap();
        live_ids.push(d.event_id.expect("durable event carries id"));
    }

    // Arrival order MUST be strictly increasing with no gaps (1..=N).
    let expected: Vec<i64> = (1..=N as i64).collect();
    assert_eq!(
        live_ids, expected,
        "live broadcast must arrive in strict event_id order with no gaps"
    );

    // The durable log MUST match exactly — replay returns the same set in
    // the same order (no event dropped at the replay seam).
    let replayed: Vec<i64> = bus
        .replay_since(session, 0)
        .await
        .unwrap()
        .iter()
        .map(|e| e.event_id.unwrap())
        .collect();
    assert_eq!(
        replayed, expected,
        "durable replay must contain every event in id order"
    );
}
