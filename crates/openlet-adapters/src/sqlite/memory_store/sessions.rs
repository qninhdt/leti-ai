//! Session-row CRUD helpers used by the `MemoryStore` trait impl.
//!
//! All helpers take `&SqlitePool` as their first argument so the trait
//! impl in `mod.rs` is a thin dispatcher.

use sqlx::SqlitePool;

use openlet_core::error::MemoryError;
use openlet_core::types::agent::AgentId;
use openlet_core::types::permission::PermissionMode;
use openlet_core::types::session::{
    SessionFilter, SessionId, SessionMeta, SessionStatus,
};

use super::super::codec::{encode_json, map_io, mode_str, now_ms, status_str};
use super::rows::row_to_session;

pub(super) async fn create_session(
    pool: &SqlitePool,
    agent_id: AgentId,
    parent: Option<SessionId>,
) -> Result<SessionId, MemoryError> {
    let id = SessionId::new();
    let now = now_ms();
    let id_str = id.to_string();
    let parent_str = parent.map(|p| p.to_string());
    let status = status_str(SessionStatus::Idle);
    let mode = mode_str(PermissionMode::default());
    let agent_str = agent_id.to_string();

    sqlx::query(
        r#"INSERT INTO sessions
            (id, agent_id, parent_session_id, status, permission_mode,
             version, created_at, updated_at, deleted_at, extensions,
             capabilities, current_agent_slug, previous_agent_slug, depth)
           VALUES (?, ?, ?, ?, ?, '0.1.0', ?, ?, NULL, 'null', '{}', NULL, NULL, 0)"#,
    )
    .bind(&id_str)
    .bind(&agent_str)
    .bind(parent_str)
    .bind(status)
    .bind(mode)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .map_err(map_io)?;

    Ok(id)
}

pub(super) async fn get_session(
    pool: &SqlitePool,
    session: SessionId,
) -> Result<Option<SessionMeta>, MemoryError> {
    let row = sqlx::query(
        r#"SELECT id, agent_id, parent_session_id, status, permission_mode,
                  version, created_at, updated_at, deleted_at, extensions,
                  capabilities, current_agent_slug, previous_agent_slug, depth
           FROM sessions WHERE id = ?"#,
    )
    .bind(session.to_string())
    .fetch_optional(pool)
    .await
    .map_err(map_io)?;

    row.map(row_to_session).transpose()
}

pub(super) async fn list_sessions(
    pool: &SqlitePool,
    filter: SessionFilter,
) -> Result<Vec<SessionMeta>, MemoryError> {
    let mut sql = String::from(
        "SELECT id, agent_id, parent_session_id, status, permission_mode, \
         version, created_at, updated_at, deleted_at, extensions, capabilities, \
         current_agent_slug, previous_agent_slug, depth \
         FROM sessions WHERE 1=1",
    );
    if !filter.include_deleted {
        sql.push_str(" AND deleted_at IS NULL");
    }
    if filter.status.is_some() {
        sql.push_str(" AND status = ?");
    }
    if filter.agent_id.is_some() {
        sql.push_str(" AND agent_id = ?");
    }
    sql.push_str(" ORDER BY created_at DESC");

    let mut q = sqlx::query(&sql);
    if let Some(s) = filter.status {
        q = q.bind(status_str(s));
    }
    let agent_str = filter.agent_id.as_ref().map(|a| a.to_string());
    if let Some(a) = agent_str.as_deref() {
        q = q.bind(a);
    }

    let rows = q.fetch_all(pool).await.map_err(map_io)?;
    rows.into_iter().map(row_to_session).collect()
}

pub(super) async fn update_status(
    pool: &SqlitePool,
    session: SessionId,
    status: SessionStatus,
) -> Result<(), MemoryError> {
    let res = sqlx::query(r#"UPDATE sessions SET status = ?, updated_at = ? WHERE id = ?"#)
        .bind(status_str(status))
        .bind(now_ms())
        .bind(session.to_string())
        .execute(pool)
        .await
        .map_err(map_io)?;

    if res.rows_affected() == 0 {
        return Err(MemoryError::SessionNotFound);
    }
    Ok(())
}

pub(super) async fn update_permission_mode(
    pool: &SqlitePool,
    session: SessionId,
    mode: PermissionMode,
) -> Result<(), MemoryError> {
    let res = sqlx::query(
        r#"UPDATE sessions SET permission_mode = ?, updated_at = ?
           WHERE id = ? AND deleted_at IS NULL"#,
    )
    .bind(mode_str(mode))
    .bind(now_ms())
    .bind(session.to_string())
    .execute(pool)
    .await
    .map_err(map_io)?;

    if res.rows_affected() == 0 {
        return Err(MemoryError::SessionNotFound);
    }
    Ok(())
}

pub(super) async fn switch_agent(
    pool: &SqlitePool,
    session: SessionId,
    agent_slug: &str,
) -> Result<(), MemoryError> {
    // Atomic SET previous := current; current := new. Done in a
    // single statement so two concurrent switch_agent calls (e.g.
    // EnterPlanMode + a stale ExitPlanMode racing) can't lose the
    // pre-swap slug. SQLite serializes writers via the db lock,
    // which collapses this read+write into a single executor pass.
    let res = sqlx::query(
        r#"UPDATE sessions
           SET previous_agent_slug = current_agent_slug,
               current_agent_slug  = ?,
               updated_at          = ?
           WHERE id = ? AND deleted_at IS NULL"#,
    )
    .bind(agent_slug)
    .bind(now_ms())
    .bind(session.to_string())
    .execute(pool)
    .await
    .map_err(map_io)?;

    if res.rows_affected() == 0 {
        return Err(MemoryError::SessionNotFound);
    }
    Ok(())
}

pub(super) async fn update_session_extensions(
    pool: &SqlitePool,
    session: SessionId,
    extensions: serde_json::Value,
) -> Result<(), MemoryError> {
    let json = encode_json(&extensions, "extensions json")?;
    let res = sqlx::query(
        r#"UPDATE sessions SET extensions = ?, updated_at = ?
           WHERE id = ? AND deleted_at IS NULL"#,
    )
    .bind(json)
    .bind(now_ms())
    .bind(session.to_string())
    .execute(pool)
    .await
    .map_err(map_io)?;

    if res.rows_affected() == 0 {
        return Err(MemoryError::SessionNotFound);
    }
    Ok(())
}

pub(super) async fn delete_session(
    pool: &SqlitePool,
    session: SessionId,
) -> Result<(), MemoryError> {
    let now = now_ms();
    let res = sqlx::query(
        r#"UPDATE sessions SET status = 'cancelled', deleted_at = ?, updated_at = ?
           WHERE id = ? AND deleted_at IS NULL"#,
    )
    .bind(now)
    .bind(now)
    .bind(session.to_string())
    .execute(pool)
    .await
    .map_err(map_io)?;

    if res.rows_affected() == 0 {
        return Err(MemoryError::SessionNotFound);
    }
    Ok(())
}

pub(super) async fn record_read(
    pool: &SqlitePool,
    session: SessionId,
    path: std::path::PathBuf,
) -> Result<(), MemoryError> {
    let path_str = path.to_string_lossy().to_string();
    sqlx::query(
        r#"INSERT INTO session_reads (session_id, path, read_at)
           VALUES (?, ?, ?)
           ON CONFLICT(session_id, path) DO UPDATE SET read_at = excluded.read_at"#,
    )
    .bind(session.to_string())
    .bind(path_str)
    .bind(now_ms())
    .execute(pool)
    .await
    .map_err(map_io)?;
    Ok(())
}
