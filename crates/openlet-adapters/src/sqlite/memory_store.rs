//! `SqliteMemoryStore` — `MemoryStore` impl backed by sqlx + SQLite.
//!
//! All write methods take `&self` and acquire from a shared `SqlitePool`.
//! Per-session monotonic `seq` is assigned in-DB to avoid races.

use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::error::MemoryError;
use openlet_core::types::agent::AgentId;
use openlet_core::types::message::{Message, MessageId, Role};
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::permission::PermissionMode;
use openlet_core::types::session::{
    SessionCapabilities, SessionFilter, SessionId, SessionMeta, SessionStatus,
};

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

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn from_ms(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now)
}

fn map_io(e: sqlx::Error) -> MemoryError {
    MemoryError::Io(e.to_string())
}

fn role_str(r: Role) -> &'static str {
    match r {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn parse_role(s: &str) -> Result<Role, MemoryError> {
    match s {
        "system" => Ok(Role::System),
        "user" => Ok(Role::User),
        "assistant" => Ok(Role::Assistant),
        "tool" => Ok(Role::Tool),
        other => Err(MemoryError::Io(format!("unknown role: {other}"))),
    }
}

fn status_str(s: SessionStatus) -> &'static str {
    match s {
        SessionStatus::Idle => "idle",
        SessionStatus::Running => "running",
        SessionStatus::Cancelling => "cancelling",
        SessionStatus::Cancelled => "cancelled",
        SessionStatus::Errored => "errored",
    }
}

fn parse_status(s: &str) -> Result<SessionStatus, MemoryError> {
    match s {
        "idle" => Ok(SessionStatus::Idle),
        "running" => Ok(SessionStatus::Running),
        "cancelling" => Ok(SessionStatus::Cancelling),
        "cancelled" => Ok(SessionStatus::Cancelled),
        "errored" => Ok(SessionStatus::Errored),
        other => Err(MemoryError::Io(format!("unknown status: {other}"))),
    }
}

fn mode_str(m: PermissionMode) -> &'static str {
    match m {
        PermissionMode::ReadOnly => "read_only",
        PermissionMode::WorkspaceWrite => "workspace_write",
        PermissionMode::Danger => "danger",
    }
}

fn parse_mode(s: &str) -> Result<PermissionMode, MemoryError> {
    match s {
        "read_only" => Ok(PermissionMode::ReadOnly),
        "workspace_write" => Ok(PermissionMode::WorkspaceWrite),
        "danger" => Ok(PermissionMode::Danger),
        other => Err(MemoryError::Io(format!("unknown mode: {other}"))),
    }
}

fn parse_uuid(s: &str) -> Result<Uuid, MemoryError> {
    Uuid::parse_str(s).map_err(|e| MemoryError::Io(format!("uuid parse: {e}")))
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
                 capabilities, current_agent_slug, previous_agent_slug)
               VALUES (?, ?, ?, ?, ?, '0.1.0', ?, ?, NULL, 'null', '{}', NULL, NULL)"#,
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
                      capabilities, current_agent_slug, previous_agent_slug
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
             current_agent_slug, previous_agent_slug \
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
        let json = serde_json::to_string(&extensions)
            .map_err(|e| MemoryError::Io(format!("extensions json: {e}")))?;
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
        let payload = serde_json::to_string(&part)
            .map_err(|e| MemoryError::Io(format!("encode part: {e}")))?;

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
        let payload = serde_json::to_string(&part)
            .map_err(|e| MemoryError::Io(format!("encode part: {e}")))?;

        let res = sqlx::query(
            r#"UPDATE parts SET kind = ?, payload = ?
               WHERE id = ? AND message_id = ?"#,
        )
        .bind(kind)
        .bind(&payload)
        .bind(part_id.to_string())
        .bind(msg.to_string())
        .execute(&self.pool)
        .await
        .map_err(map_io)?;

        if res.rows_affected() == 0 {
            self.append_part(msg, part).await.map(|_| ())
        } else {
            Ok(())
        }
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
            let part: Part = serde_json::from_str(&payload)
                .map_err(|e| MemoryError::Io(format!("decode part: {e}")))?;
            out.push(part);
        }
        Ok(out)
    }
}

fn part_kind(part: &Part) -> &'static str {
    match part {
        Part::Text { .. } => "text",
        Part::Reasoning { .. } => "reasoning",
        Part::ToolCall { .. } => "tool_call",
        Part::ToolResult { .. } => "tool_result",
        Part::Image { .. } => "image",
        Part::Document { .. } => "document",
        Part::StepStart { .. } => "step_start",
        Part::StepFinish { .. } => "step_finish",
        Part::Compaction { .. } => "compaction",
        Part::Plan { .. } => "plan",
    }
}

fn row_to_session(row: sqlx::sqlite::SqliteRow) -> Result<SessionMeta, MemoryError> {
    let id_str: String = row.try_get("id").map_err(map_io)?;
    let agent_id_str: String = row.try_get("agent_id").map_err(map_io)?;
    let parent: Option<String> = row.try_get("parent_session_id").map_err(map_io)?;
    let status: String = row.try_get("status").map_err(map_io)?;
    let mode: String = row.try_get("permission_mode").map_err(map_io)?;
    let version: String = row.try_get("version").map_err(map_io)?;
    let created_at: i64 = row.try_get("created_at").map_err(map_io)?;
    let updated_at: i64 = row.try_get("updated_at").map_err(map_io)?;
    let deleted_at: Option<i64> = row.try_get("deleted_at").map_err(map_io)?;
    let extensions: String = row.try_get("extensions").map_err(map_io)?;
    let extensions = serde_json::from_str(&extensions)
        .map_err(|e| MemoryError::Io(format!("extensions json: {e}")))?;
    let capabilities: String = row.try_get("capabilities").map_err(map_io)?;
    let capabilities: SessionCapabilities = serde_json::from_str(&capabilities)
        .map_err(|e| MemoryError::Io(format!("capabilities json: {e}")))?;
    let current_agent_slug: Option<String> = row.try_get("current_agent_slug").map_err(map_io)?;
    let previous_agent_slug: Option<String> = row.try_get("previous_agent_slug").map_err(map_io)?;

    Ok(SessionMeta {
        id: SessionId(parse_uuid(&id_str)?),
        agent_id: AgentId(parse_uuid(&agent_id_str)?),
        status: parse_status(&status)?,
        permission_mode: parse_mode(&mode)?,
        parent_session_id: parent.map(|p| parse_uuid(&p).map(SessionId)).transpose()?,
        created_at: from_ms(created_at),
        updated_at: from_ms(updated_at),
        deleted_at: deleted_at.map(from_ms),
        version,
        extensions,
        capabilities,
        current_agent_slug,
        previous_agent_slug,
    })
}

fn row_to_message(row: sqlx::sqlite::SqliteRow) -> Result<Message, MemoryError> {
    let id_str: String = row.try_get("id").map_err(map_io)?;
    let session_str: String = row.try_get("session_id").map_err(map_io)?;
    let role: String = row.try_get("role").map_err(map_io)?;
    let created_at: i64 = row.try_get("created_at").map_err(map_io)?;

    Ok(Message {
        id: MessageId(parse_uuid(&id_str)?),
        session_id: SessionId(parse_uuid(&session_str)?),
        role: parse_role(&role)?,
        created_at: from_ms(created_at),
    })
}
