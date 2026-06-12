//! Round-trip tests for `SqliteMemoryStore` against an in-memory pool.

use chrono::Utc;
use openlet_adapters::sqlite::{SqliteMemoryStore, open_in_memory};
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::types::agent::AgentId;
use openlet_core::types::message::{Message, MessageId, Role};
use openlet_core::types::pagination::Page;
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

    let live_only = store.list_sessions(SessionFilter::default()).await.unwrap();
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

#[tokio::test]
async fn create_session_with_meta_persists_verbatim_and_accepts_children() {
    // Regression: subagent spawning persists a pre-built child SessionMeta
    // (with a caller-allocated id + non-zero depth) and then seeds messages
    // keyed on that id. The old code called create_session, which minted a
    // FRESH id and reset depth to 0 — the seed append then hit a foreign-key
    // violation against a row that didn't exist. create_session_with_meta
    // must persist the row verbatim so the id/depth survive and child writes
    // succeed.
    use openlet_core::types::permission::PermissionMode;
    use openlet_core::types::session::{SessionId, SessionMeta};

    let pool = open_in_memory().await.expect("pool");
    let store = SqliteMemoryStore::new(pool);

    let agent = AgentId::new();
    let parent = store.create_session(agent, None).await.unwrap();
    let child_id = SessionId::new();
    let now = Utc::now();
    let child = SessionMeta {
        id: child_id,
        agent_id: agent,
        status: SessionStatus::Running,
        permission_mode: PermissionMode::default(),
        parent_session_id: Some(parent),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        version: "0.1.0".into(),
        extensions: serde_json::Value::Null,
        capabilities: Default::default(),
        current_agent_slug: Some("indexer".into()),
        previous_agent_slug: None,
        depth: 2,
        model: None,
    };

    let returned = store.create_session_with_meta(child.clone()).await.unwrap();
    assert_eq!(returned, child_id, "returned id must equal the supplied id");

    let got = store.get_session(child_id).await.unwrap().expect("present");
    assert_eq!(got.id, child_id);
    assert_eq!(got.parent_session_id, Some(parent));
    assert_eq!(
        got.depth, 2,
        "depth must be preserved for the grandchild guard"
    );
    assert_eq!(got.status, SessionStatus::Running);
    assert_eq!(got.current_agent_slug.as_deref(), Some("indexer"));

    // The FK-critical path: a message append against the child id must
    // succeed (it would fail if the row were never inserted).
    let msg = Message {
        id: MessageId::new(),
        session_id: child_id,
        role: Role::User,
        created_at: Utc::now(),
    };
    store
        .append_message(child_id, msg)
        .await
        .expect("append against persisted child session must succeed");
}

#[tokio::test]
async fn session_model_round_trips() {
    use openlet_core::types::permission::PermissionMode;
    use openlet_core::types::session::{SessionId, SessionMeta};

    let pool = open_in_memory().await.expect("pool");
    let store = SqliteMemoryStore::new(pool);

    let agent = AgentId::new();
    let id = SessionId::new();
    let now = Utc::now();
    let meta = SessionMeta {
        id,
        agent_id: agent,
        status: SessionStatus::Idle,
        permission_mode: PermissionMode::default(),
        parent_session_id: None,
        created_at: now,
        updated_at: now,
        deleted_at: None,
        version: "0.1.0".into(),
        extensions: serde_json::Value::Null,
        capabilities: Default::default(),
        current_agent_slug: None,
        previous_agent_slug: None,
        depth: 0,
        model: Some("anthropic/claude-opus-4-8".into()),
    };
    store.create_session_with_meta(meta).await.unwrap();

    let got = store.get_session(id).await.unwrap().expect("present");
    assert_eq!(got.model.as_deref(), Some("anthropic/claude-opus-4-8"));
}

#[tokio::test]
async fn old_format_session_loads_model_as_none() {
    // M11: a row written before the `model` column existed (here: a row
    // created via `create_session`, which leaves model NULL) must load
    // with `model = None` rather than failing the decode.
    let pool = open_in_memory().await.expect("pool");
    let store = SqliteMemoryStore::new(pool);

    let agent = AgentId::new();
    let id = store.create_session(agent, None).await.expect("create");
    let got = store.get_session(id).await.unwrap().expect("present");
    assert!(
        got.model.is_none(),
        "a session created without a model override must load as None"
    );
}

#[tokio::test]
async fn list_sessions_paged_walks_pages() {
    let pool = open_in_memory().await.expect("pool");
    let store = SqliteMemoryStore::new(pool);
    let a = AgentId::new();
    for _ in 0..5 {
        store.create_session(a, None).await.unwrap();
    }

    // Page 1: 2 of 5 → next cursor present.
    let p1 = store
        .list_sessions_paged(SessionFilter::default(), Page::first(2))
        .await
        .unwrap();
    assert_eq!(p1.items.len(), 2);
    assert!(p1.next_cursor.is_some());

    // Page 2: next 2.
    let p2 = store
        .list_sessions_paged(
            SessionFilter::default(),
            Page {
                cursor: p1.next_cursor,
                limit: 2,
            },
        )
        .await
        .unwrap();
    assert_eq!(p2.items.len(), 2);
    assert!(p2.next_cursor.is_some());

    // Page 3: last 1 → no further cursor.
    let p3 = store
        .list_sessions_paged(
            SessionFilter::default(),
            Page {
                cursor: p2.next_cursor,
                limit: 2,
            },
        )
        .await
        .unwrap();
    assert_eq!(p3.items.len(), 1);
    assert_eq!(p3.next_cursor, None);

    // Paged walk must cover the same set as the unbounded list.
    let unbounded = store.list_sessions(SessionFilter::default()).await.unwrap();
    assert_eq!(
        p1.items.len() + p2.items.len() + p3.items.len(),
        unbounded.len()
    );
}

#[tokio::test]
async fn list_parts_scoped_to_session() {
    let pool = open_in_memory().await.expect("pool");
    let store = SqliteMemoryStore::new(pool);

    let agent = AgentId::new();
    let session_a = store.create_session(agent, None).await.unwrap();
    let session_b = store.create_session(agent, None).await.unwrap();

    let mid = MessageId::new();
    let msg = Message {
        id: mid,
        session_id: session_a,
        role: Role::Assistant,
        created_at: Utc::now(),
    };
    store.append_message(session_a, msg).await.unwrap();

    let pid = PartId::new();
    let part = Part::Text {
        id: pid,
        text: "hello".into(),
    };
    store.append_part(mid, part).await.unwrap();

    // Correct session returns the part.
    let parts = store.list_parts(session_a, mid).await.unwrap();
    assert_eq!(parts.len(), 1);

    // Wrong session returns empty — the message belongs to session_a, not session_b.
    let empty = store.list_parts(session_b, mid).await.unwrap();
    assert!(empty.is_empty(), "list_parts must enforce session scoping");
}

#[tokio::test]
async fn list_messages_paged_matches_unbounded_order() {
    let pool = open_in_memory().await.expect("pool");
    let store = SqliteMemoryStore::new(pool);
    let session = store.create_session(AgentId::new(), None).await.unwrap();
    for _ in 0..3 {
        let msg = Message {
            id: MessageId::new(),
            session_id: session,
            role: Role::User,
            created_at: Utc::now(),
        };
        store.append_message(session, msg).await.unwrap();
    }

    let unbounded = store.list_messages(session).await.unwrap();
    let mut walked = Vec::new();
    let mut cursor = None;
    loop {
        let page = store
            .list_messages_paged(session, Page { cursor, limit: 2 })
            .await
            .unwrap();
        walked.extend(page.items.iter().map(|m| m.id));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    let expected: Vec<_> = unbounded.iter().map(|m| m.id).collect();
    assert_eq!(walked, expected, "paged walk must equal unbounded order");
}
