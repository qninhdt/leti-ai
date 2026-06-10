//! `SqliteMemoryStore` per-session monotonic `seq` under
//! concurrent appends.
//!
//! `append_message` and `append_part` compute their seq via
//! `SELECT COALESCE(MAX(seq), 0) + 1` against a UNIQUE index. SQLite
//! serialises writers via the database lock; the test asserts the
//! observable contract: 50 concurrent appends produce seq values
//! `1..=50` exactly, with no duplicates and no holes.
//!
//! `:memory:` is used — existing
//! sqlite_memory_store.rs proves migrations work in :memory:; if a
//! future race needs file-backed semantics, switch to NamedTempFile.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::Utc;
use openlet_adapters::sqlite::{SqliteMemoryStore, open_in_memory};
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::types::agent::AgentId;
use openlet_core::types::message::{Message, MessageId, Role};
use openlet_core::types::part::{Part, PartId};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn append_message_seq_is_dense_under_concurrent_appends() {
    const N: usize = 50;

    let pool = open_in_memory().await.expect("pool");
    let store = Arc::new(SqliteMemoryStore::new(pool));
    let session = store.create_session(AgentId::new(), None).await.unwrap();

    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            store
                .append_message(
                    session,
                    Message {
                        id: MessageId::new(),
                        session_id: session,
                        role: Role::User,
                        created_at: Utc::now(),
                    },
                )
                .await
                .expect("append_message")
        }));
    }
    for h in handles {
        let _ = h.await.unwrap();
    }

    let listed = store.list_messages(session).await.unwrap();
    assert_eq!(listed.len(), N, "all appends persisted");

    // `list_messages` orders by seq ASC. After de-duping there must be
    // exactly N distinct ids, and the position must equal the seq.
    let unique_ids: HashSet<MessageId> = listed.iter().map(|m| m.id).collect();
    assert_eq!(
        unique_ids.len(),
        N,
        "no duplicate ids — every insert kept its row"
    );

    // The store doesn't expose seq via list_messages, but it sorts by
    // seq, so list order = seq order. Test the implicit contract by
    // querying the seq column directly.
    let pool = store.pool();
    let rows: Vec<(i64,)> =
        sqlx::query_as("SELECT seq FROM messages WHERE session_id = ? ORDER BY seq ASC")
            .bind(session.0.to_string())
            .fetch_all(pool)
            .await
            .unwrap();
    let seqs: Vec<i64> = rows.into_iter().map(|(s,)| s).collect();
    assert_eq!(
        seqs,
        (1..=N as i64).collect::<Vec<_>>(),
        "seq values must be exactly 1..=N — no gaps, no duplicates"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn append_part_seq_is_dense_under_concurrent_appends_per_message() {
    const N: usize = 25;

    let pool = open_in_memory().await.expect("pool");
    let store = Arc::new(SqliteMemoryStore::new(pool));
    let session = store.create_session(AgentId::new(), None).await.unwrap();
    let mid = MessageId::new();
    store
        .append_message(
            session,
            Message {
                id: mid,
                session_id: session,
                role: Role::Assistant,
                created_at: Utc::now(),
            },
        )
        .await
        .unwrap();

    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let store = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            store
                .append_part(
                    mid,
                    Part::Text {
                        id: PartId::new(),
                        text: format!("p{i}"),
                    },
                )
                .await
                .expect("append_part")
        }));
    }
    for h in handles {
        let _ = h.await.unwrap();
    }

    let pool = store.pool();
    let rows: Vec<(i64,)> =
        sqlx::query_as("SELECT seq FROM parts WHERE message_id = ? ORDER BY seq ASC")
            .bind(mid.0.to_string())
            .fetch_all(pool)
            .await
            .unwrap();
    let seqs: Vec<i64> = rows.into_iter().map(|(s,)| s).collect();
    assert_eq!(
        seqs,
        (1..=N as i64).collect::<Vec<_>>(),
        "part seq must be dense 1..=N within a single message"
    );
}
