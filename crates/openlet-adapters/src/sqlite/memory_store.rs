//! `SqliteMemoryStore` — `MemoryStore` impl backed by sqlx + SQLite.
//!
//! All write methods take `&self` and acquire from a shared `SqlitePool`.
//! Per-session monotonic `seq` is assigned in-DB to avoid races.

use std::path::PathBuf;

use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::error::MemoryError;
use openlet_core::types::agent::AgentId;
use openlet_core::types::message::{Message, MessageId};
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::permission::PermissionMode;
use openlet_core::types::session::{SessionFilter, SessionId, SessionMeta, SessionStatus};

use super::codec::{
    decode_json, encode_json, map_io, mode_str, now_ms, part_kind, role_str, status_str,
};
use super::rows::{row_to_message, row_to_session};

#[derive(Debug, Clone)]
pub struct SqliteMemoryStore {
    pool: SqlitePool,
}

impl SqliteMemoryStore {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[async_trait]
impl MemoryStore for SqliteMemoryStore {
    async fn create_session(
        &self,
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
        .execute(&self.pool)
        .await
        .map_err(map_io)?;

        Ok(id)
    }

    async fn get_session(&self, session: SessionId) -> Result<Option<SessionMeta>, MemoryError> {
        let row = sqlx::query(
            r#"SELECT id, agent_id, parent_session_id, status, permission_mode,
                      version, created_at, updated_at, deleted_at, extensions,
                      capabilities, current_agent_slug, previous_agent_slug, depth
               FROM sessions WHERE id = ?"#,
        )
        .bind(session.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(map_io)?;

        row.map(row_to_session).transpose()
    }

    async fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, MemoryError> {
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

        let rows = q.fetch_all(&self.pool).await.map_err(map_io)?;
        rows.into_iter().map(row_to_session).collect()
    }

    async fn update_status(
        &self,
        session: SessionId,
        status: SessionStatus,
        _reason: &str,
    ) -> Result<(), MemoryError> {
        let res = sqlx::query(r#"UPDATE sessions SET status = ?, updated_at = ? WHERE id = ?"#)
            .bind(status_str(status))
            .bind(now_ms())
            .bind(session.to_string())
            .execute(&self.pool)
            .await
            .map_err(map_io)?;

        if res.rows_affected() == 0 {
            return Err(MemoryError::SessionNotFound);
        }
        Ok(())
    }

    async fn update_permission_mode(
        &self,
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
        .execute(&self.pool)
        .await
        .map_err(map_io)?;

        if res.rows_affected() == 0 {
            return Err(MemoryError::SessionNotFound);
        }
        Ok(())
    }

    async fn switch_agent(&self, session: SessionId, agent_slug: &str) -> Result<(), MemoryError> {
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
        .execute(&self.pool)
        .await
        .map_err(map_io)?;

        if res.rows_affected() == 0 {
            return Err(MemoryError::SessionNotFound);
        }
        Ok(())
    }

    async fn update_session_extensions(
        &self,
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
        .execute(&self.pool)
        .await
        .map_err(map_io)?;

        if res.rows_affected() == 0 {
            return Err(MemoryError::SessionNotFound);
        }
        Ok(())
    }

    async fn delete_session(&self, session: SessionId) -> Result<(), MemoryError> {
        let now = now_ms();
        let res = sqlx::query(
            r#"UPDATE sessions SET status = 'cancelled', deleted_at = ?, updated_at = ?
               WHERE id = ? AND deleted_at IS NULL"#,
        )
        .bind(now)
        .bind(now)
        .bind(session.to_string())
        .execute(&self.pool)
        .await
        .map_err(map_io)?;

        if res.rows_affected() == 0 {
            return Err(MemoryError::SessionNotFound);
        }
        Ok(())
    }

    async fn append_message(
        &self,
        session: SessionId,
        msg: Message,
    ) -> Result<MessageId, MemoryError> {
        // Single atomic INSERT with subquery so concurrent appenders
        // can't both compute the same MAX(seq) and trip UNIQUE(session_id,
        // seq). SQLite serializes writers via the db lock; this collapses
        // the read+write into one statement so they can't interleave.
        // Closes B/I2.
        let mut tx = self.pool.begin().await.map_err(map_io)?;
        let id = msg.id;
        sqlx::query(
            r#"INSERT INTO messages (id, session_id, role, seq, created_at, meta)
               VALUES (
                 ?, ?, ?,
                 (SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE session_id = ?),
                 ?, '{}'
               )"#,
        )
        .bind(id.to_string())
        .bind(session.to_string())
        .bind(role_str(msg.role))
        .bind(session.to_string())
        .bind(msg.created_at.timestamp_millis())
        .execute(&mut *tx)
        .await
        .map_err(map_io)?;

        sqlx::query(r#"UPDATE sessions SET updated_at = ? WHERE id = ?"#)
            .bind(now_ms())
            .bind(session.to_string())
            .execute(&mut *tx)
            .await
            .map_err(map_io)?;

        tx.commit().await.map_err(map_io)?;
        Ok(id)
    }

    async fn append_part(&self, msg: MessageId, part: Part) -> Result<PartId, MemoryError> {
        let id = part.id();
        let kind = part_kind(&part);
        let payload = encode_json(&part, "encode part")?;

        // Single atomic INSERT — see append_message rationale (B/I2).
        let mut tx = self.pool.begin().await.map_err(map_io)?;

        sqlx::query(
            r#"INSERT INTO parts (id, message_id, seq, kind, payload)
               VALUES (
                 ?, ?,
                 (SELECT COALESCE(MAX(seq), 0) + 1 FROM parts WHERE message_id = ?),
                 ?, ?
               )"#,
        )
        .bind(id.to_string())
        .bind(msg.to_string())
        .bind(msg.to_string())
        .bind(kind)
        .bind(&payload)
        .execute(&mut *tx)
        .await
        .map_err(map_io)?;

        tx.commit().await.map_err(map_io)?;
        Ok(id)
    }

    async fn upsert_part(
        &self,
        msg: MessageId,
        part_id: PartId,
        part: Part,
    ) -> Result<(), MemoryError> {
        let kind = part_kind(&part);
        let payload = encode_json(&part, "encode part")?;

        // INSERT ON CONFLICT collapses the previous UPDATE-then-fallback-INSERT
        // pattern into a single statement. The old form raced: two concurrent
        // upserts on the same fresh (msg, part_id) could both UPDATE-zero,
        // both fall through to INSERT, and the second would PK-conflict.
        // `seq` is required NOT NULL on first insert; for the upsert path
        // we only assign it when the row doesn't already exist via the
        // COALESCE on excluded.seq. New rows get the next per-message seq.
        let next_seq: i64 = sqlx::query_scalar(
            r#"SELECT COALESCE(MAX(seq), -1) + 1 FROM parts WHERE message_id = ?"#,
        )
        .bind(msg.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(map_io)?;

        sqlx::query(
            r#"INSERT INTO parts (id, message_id, seq, kind, payload)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET
                   kind = excluded.kind,
                   payload = excluded.payload"#,
        )
        .bind(part_id.to_string())
        .bind(msg.to_string())
        .bind(next_seq)
        .bind(kind)
        .bind(&payload)
        .execute(&self.pool)
        .await
        .map_err(map_io)?;

        Ok(())
    }

    async fn list_messages(&self, session: SessionId) -> Result<Vec<Message>, MemoryError> {
        let rows = sqlx::query(
            r#"SELECT id, session_id, role, created_at FROM messages
               WHERE session_id = ? ORDER BY seq ASC"#,
        )
        .bind(session.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(map_io)?;

        rows.into_iter().map(row_to_message).collect()
    }

    async fn record_read(&self, session: SessionId, path: PathBuf) -> Result<(), MemoryError> {
        let path_str = path.to_string_lossy().to_string();
        sqlx::query(
            r#"INSERT INTO session_reads (session_id, path, read_at)
               VALUES (?, ?, ?)
               ON CONFLICT(session_id, path) DO UPDATE SET read_at = excluded.read_at"#,
        )
        .bind(session.to_string())
        .bind(path_str)
        .bind(now_ms())
        .execute(&self.pool)
        .await
        .map_err(map_io)?;
        Ok(())
    }

    async fn list_parts(
        &self,
        _session: SessionId,
        msg: MessageId,
    ) -> Result<Vec<Part>, MemoryError> {
        let rows =
            sqlx::query(r#"SELECT payload FROM parts WHERE message_id = ? ORDER BY seq ASC"#)
                .bind(msg.to_string())
                .fetch_all(&self.pool)
                .await
                .map_err(map_io)?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let payload: String = row.try_get("payload").map_err(map_io)?;
            let part: Part = decode_json(&payload, "decode part")?;
            out.push(part);
        }
        Ok(out)
    }
}
