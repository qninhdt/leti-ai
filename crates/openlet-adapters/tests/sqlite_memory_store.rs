//! Round-trip tests for `SqliteMemoryStore` against an in-memory pool.

use chrono::Utc;
use openlet_adapters::sqlite::{SqliteMemoryStore, open_in_memory};
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::types::agent::AgentId;
use openlet_core::types::message::{Message, MessageId, Role};
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::session::{SessionFilter, SessionStatus};

#[tokio::test]
async fn create_and_get_session() {
    let pool = open_in_memory().await.expect("pool");
    let store = SqliteMemoryStore::new(pool);

    let agent = AgentId::new();
    let id = store.create_session(agent, None).await.expect("create");
    let meta = store
        .get_session(id)
        .await
        .expect("get")
        .expect("session present");
    assert_eq!(meta.id, id);
    assert_eq!(meta.agent_id, agent);
    assert_eq!(meta.status, SessionStatus::Idle);
    assert!(meta.deleted_at.is_none());
}

#[tokio::test]
async fn list_filters_deleted() {
    let pool = open_in_memory().await.expect("pool");
    let store = SqliteMemoryStore::new(pool);
    let a = AgentId::new();
    let alive = store.create_session(a, None).await.unwrap();
    let dead = store.create_session(a, None).await.unwrap();
    store.delete_session(dead).await.unwrap();

    let live_only = store
        .list_sessions(SessionFilter::default())
        .await
        .unwrap();
    assert!(live_only.iter().any(|m| m.id == alive));
    assert!(!live_only.iter().any(|m| m.id == dead));

    let all = store
        .list_sessions(SessionFilter {
            include_deleted: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn append_messages_keeps_seq_order() {
    let pool = open_in_memory().await.expect("pool");
    let store = SqliteMemoryStore::new(pool);
    let session = store.create_session(AgentId::new(), None).await.unwrap();

    for role in [Role::User, Role::Assistant, Role::Tool, Role::User] {
        let msg = Message {
            id: MessageId::new(),
            session_id: session,
            role,
            created_at: Utc::now(),
        };
        store.append_message(session, msg).await.unwrap();
    }

    let listed = store.list_messages(session).await.unwrap();
    assert_eq!(listed.len(), 4);
    assert_eq!(listed[0].role, Role::User);
    assert_eq!(listed[1].role, Role::Assistant);
    assert_eq!(listed[2].role, Role::Tool);
    assert_eq!(listed[3].role, Role::User);
}

#[tokio::test]
async fn append_and_upsert_part() {
    let pool = open_in_memory().await.expect("pool");
    let store = SqliteMemoryStore::new(pool);
    let session = store.create_session(AgentId::new(), None).await.unwrap();

    let mid = MessageId::new();
    let msg = Message {
        id: mid,
        session_id: session,
        role: Role::Assistant,
        created_at: Utc::now(),
    };
    store.append_message(session, msg).await.unwrap();

    let pid = PartId::new();
    let initial = Part::Text {
        id: pid,
        text: "hi".into(),
    };
    store.append_part(mid, initial).await.unwrap();

    let updated = Part::Text {
        id: pid,
        text: "hi there".into(),
    };
    store.upsert_part(mid, pid, updated).await.unwrap();
}

#[tokio::test]
async fn record_read_idempotent() {
    let pool = open_in_memory().await.expect("pool");
    let store = SqliteMemoryStore::new(pool);
    let session = store.create_session(AgentId::new(), None).await.unwrap();

    let path = std::path::PathBuf::from("/tmp/file.rs");
    store.record_read(session, path.clone()).await.unwrap();
    store.record_read(session, path).await.unwrap();
}

#[tokio::test]
async fn migration_idempotent() {
    let pool = open_in_memory().await.expect("pool");
    openlet_adapters::sqlite::run_migrations(&pool)
        .await
        .expect("re-run migrations no-op");
}
