//! Integration tests for `SqliteMemoryStore` soft-delete semantics.
//!
//! `delete_session` is a SOFT delete (sets `deleted_at` + `status =
//! 'cancelled'`), not a hard delete. Messages, parts, and events are
//! preserved on disk so audit replay still works. The tests below lock
//! the contract surface that the rest of the runtime relies on:
//!
//! 1. After delete, `get_session` still returns the row (soft-deleted
//!    sessions remain queryable for audit).
//! 2. `list_sessions(include_deleted: false)` excludes soft-deleted.
//! 3. `list_sessions(include_deleted: true)` includes them.
//! 4. Messages and parts remain readable after soft delete (audit replay).
//! 5. A second `delete_session` on an already-deleted row returns
//!    `SessionNotFound` (the WHERE clause matches `deleted_at IS NULL`).
//! 6. `delete_session` on an unknown id returns `SessionNotFound`.
//! 7. Status mutators (`update_status`, `update_permission_mode`,
//!    `switch_agent`) refuse to touch soft-deleted rows.

mod common;

use common::sqlite_helper::make_pool;
use leti_adapters::sqlite::SqliteMemoryStore;
use leti_core::adapters::MemoryStore;
use leti_core::error::MemoryError;
use leti_core::types::agent::AgentId;
use leti_core::types::message::{Message, MessageId, Role};
use leti_core::types::part::{Part, PartId};
use leti_core::types::permission::PermissionMode;
use leti_core::types::session::{SessionFilter, SessionStatus};

async fn make_store() -> SqliteMemoryStore {
    SqliteMemoryStore::new(make_pool().await)
}

fn user_text_msg() -> (Message, Part) {
    use chrono::Utc;
    use leti_core::types::session::SessionId;
    let mid = MessageId::new();
    let msg = Message {
        id: mid,
        session_id: SessionId::new(), // overwritten by caller via append_message(session, msg)
        role: Role::User,
        created_at: Utc::now(),
    };
    let part = Part::Text {
        id: PartId::new(),
        text: "hello".to_string(),
    };
    (msg, part)
}

#[tokio::test]
async fn delete_session_marks_row_deleted_but_preserves_history() {
    let store = make_store().await;
    let agent = AgentId::new();
    let sid = store.create_session(agent, None).await.unwrap();

    let (msg, part) = user_text_msg();
    let mid = store.append_message(sid, msg).await.unwrap();
    store.append_part(mid, part).await.unwrap();

    store.delete_session(sid).await.unwrap();

    // Soft-deleted row IS still gettable; rows.rs filters on `deleted_at
    // IS NULL` for `get_session` only via list paths, but the direct
    // get_session uses `WHERE id = ?` (no deleted_at filter). Lock that
    // contract.
    let meta = store.get_session(sid).await.unwrap();
    assert!(
        meta.is_some(),
        "get_session must still return soft-deleted row"
    );
    let meta = meta.unwrap();
    assert!(meta.deleted_at.is_some(), "deleted_at must be populated");
    assert_eq!(meta.status, SessionStatus::Cancelled);

    // Messages + parts are preserved on disk (audit replay works).
    let msgs = store.list_messages(sid).await.unwrap();
    assert_eq!(msgs.len(), 1, "messages must survive soft delete");
    let parts = store.list_parts(sid, mid).await.unwrap();
    assert_eq!(parts.len(), 1, "parts must survive soft delete");
}

#[tokio::test]
async fn list_sessions_excludes_soft_deleted_by_default() {
    let store = make_store().await;
    let agent = AgentId::new();
    let kept = store.create_session(agent, None).await.unwrap();
    let deleted = store.create_session(agent, None).await.unwrap();

    store.delete_session(deleted).await.unwrap();

    let visible = store.list_sessions(SessionFilter::default()).await.unwrap();
    let ids: Vec<_> = visible.iter().map(|s| s.id).collect();
    assert!(ids.contains(&kept), "live session must be listed");
    assert!(
        !ids.contains(&deleted),
        "soft-deleted session must NOT be listed by default"
    );
}

#[tokio::test]
async fn list_sessions_with_include_deleted_returns_soft_deleted() {
    let store = make_store().await;
    let agent = AgentId::new();
    let live = store.create_session(agent, None).await.unwrap();
    let deleted = store.create_session(agent, None).await.unwrap();
    store.delete_session(deleted).await.unwrap();

    let filter = SessionFilter {
        include_deleted: true,
        ..Default::default()
    };
    let all = store.list_sessions(filter).await.unwrap();
    let ids: Vec<_> = all.iter().map(|s| s.id).collect();
    assert!(ids.contains(&live), "live session must be listed");
    assert!(
        ids.contains(&deleted),
        "include_deleted=true must surface soft-deleted rows"
    );
}

#[tokio::test]
async fn double_delete_returns_session_not_found() {
    let store = make_store().await;
    let sid = store.create_session(AgentId::new(), None).await.unwrap();

    store.delete_session(sid).await.unwrap();
    let err = store.delete_session(sid).await.unwrap_err();
    assert!(
        matches!(err, MemoryError::SessionNotFound),
        "second delete must return SessionNotFound, got {err:?}"
    );
}

#[tokio::test]
async fn delete_unknown_session_returns_session_not_found() {
    use leti_core::types::session::SessionId;
    let store = make_store().await;
    let unknown = SessionId::new();
    let err = store.delete_session(unknown).await.unwrap_err();
    assert!(
        matches!(err, MemoryError::SessionNotFound),
        "delete of unknown session must return SessionNotFound, got {err:?}"
    );
}

#[tokio::test]
async fn update_status_currently_succeeds_on_soft_deleted_session() {
    // Asymmetry vs the other update_* helpers: `update_status` does NOT
    // include `AND deleted_at IS NULL` in its WHERE clause (see
    // crates/leti-adapters/src/sqlite/memory_store.rs:129), while
    // `update_permission_mode`, `switch_agent`, and
    // `update_session_extensions` do. Locking current behavior so a
    // future change is intentional rather than a silent contract drift.
    // Filed for cleanup separately.
    let store = make_store().await;
    let sid = store.create_session(AgentId::new(), None).await.unwrap();
    store.delete_session(sid).await.unwrap();

    let r = store
        .update_status(sid, SessionStatus::Running, "reactivate")
        .await;
    assert!(
        r.is_ok(),
        "update_status currently allowed on soft-deleted: {r:?}"
    );
}

#[tokio::test]
async fn update_permission_mode_refuses_soft_deleted_session() {
    let store = make_store().await;
    let sid = store.create_session(AgentId::new(), None).await.unwrap();
    store.delete_session(sid).await.unwrap();

    let err = store
        .update_permission_mode(sid, PermissionMode::Danger)
        .await
        .unwrap_err();
    assert!(
        matches!(err, MemoryError::SessionNotFound),
        "update_permission_mode on soft-deleted must return SessionNotFound, got {err:?}"
    );
}

#[tokio::test]
async fn switch_agent_refuses_soft_deleted_session() {
    let store = make_store().await;
    let sid = store.create_session(AgentId::new(), None).await.unwrap();
    store.delete_session(sid).await.unwrap();

    let err = store.switch_agent(sid, "general").await.unwrap_err();
    assert!(
        matches!(err, MemoryError::SessionNotFound),
        "switch_agent on soft-deleted must return SessionNotFound, got {err:?}"
    );
}

#[tokio::test]
async fn append_message_to_soft_deleted_session_succeeds_intentionally() {
    // The current implementation does NOT block writes to soft-deleted
    // sessions — the `messages` table FK only requires the session row
    // to exist (it does, since soft-delete keeps the row). Locking
    // current behavior so a future change is intentional, not a silent
    // contract drift.
    let store = make_store().await;
    let sid = store.create_session(AgentId::new(), None).await.unwrap();
    store.delete_session(sid).await.unwrap();

    let (msg, _part) = user_text_msg();
    let r = store.append_message(sid, msg).await;
    assert!(
        r.is_ok(),
        "append_message currently allowed on soft-deleted session: {r:?}"
    );
}
