//! `SqliteMemoryStore` — `MemoryStore` impl backed by sqlx + SQLite.
//!
//! All write methods take `&self` and acquire from a shared `SqlitePool`.
//! Per-session monotonic `seq` is assigned in-DB to avoid races.

use std::path::PathBuf;

use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

use openlet_core::adapters::memory_store::{
    BackgroundTaskSettlement, ClaimedBackgroundTaskSettlement, MemoryStore,
};
use openlet_core::error::MemoryError;
use openlet_core::types::agent::AgentId;
use openlet_core::types::message::{Message, MessageId};
use openlet_core::types::pagination::{Page, PageResult};
use openlet_core::types::part::{Part, PartId, ReminderKind};
use openlet_core::types::permission::PermissionMode;
use openlet_core::types::session::{SessionFilter, SessionId, SessionMeta, SessionStatus};

use super::codec::{
    decode_json, encode_json, map_io, mode_str, now_ms, part_kind, role_str, status_str,
};
use super::memory_queries::session_filter_clause;
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
                 capabilities, current_agent_slug, previous_agent_slug, depth, model)
               VALUES (?, ?, ?, ?, ?, '0.1.0', ?, ?, NULL, 'null', '{}', NULL, NULL, 0, NULL)"#,
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

    async fn create_session_with_meta(&self, meta: SessionMeta) -> Result<SessionId, MemoryError> {
        // Persist the caller-supplied row verbatim — id, depth,
        // permission_mode, status, and parent link are all preserved.
        // Subagent spawning relies on this so the child id seeded
        // messages reference actually exists, and so `depth` survives for
        // the grandchild depth-limit guard.
        let id = meta.id;
        let extensions = encode_json(&meta.extensions, "extensions json")?;
        let capabilities = encode_json(&meta.capabilities, "capabilities json")?;

        sqlx::query(
            r#"INSERT INTO sessions
                (id, agent_id, parent_session_id, status, permission_mode,
                 version, created_at, updated_at, deleted_at, extensions,
                 capabilities, current_agent_slug, previous_agent_slug, depth, model)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(id.to_string())
        .bind(meta.agent_id.to_string())
        .bind(meta.parent_session_id.map(|p| p.to_string()))
        .bind(status_str(meta.status))
        .bind(mode_str(meta.permission_mode))
        .bind(&meta.version)
        .bind(meta.created_at.timestamp_millis())
        .bind(meta.updated_at.timestamp_millis())
        .bind(meta.deleted_at.map(|d| d.timestamp_millis()))
        .bind(extensions)
        .bind(capabilities)
        .bind(meta.current_agent_slug.as_deref())
        .bind(meta.previous_agent_slug.as_deref())
        .bind(i64::from(meta.depth))
        .bind(meta.model.as_deref())
        .execute(&self.pool)
        .await
        .map_err(map_io)?;

        Ok(id)
    }

    async fn get_session(&self, session: SessionId) -> Result<Option<SessionMeta>, MemoryError> {
        let row = sqlx::query(
            r#"SELECT id, agent_id, parent_session_id, status, permission_mode,
                      version, created_at, updated_at, deleted_at, extensions,
                      capabilities, current_agent_slug, previous_agent_slug, depth, model
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
             current_agent_slug, previous_agent_slug, depth, model \
             FROM sessions WHERE 1=1",
        );
        let (clauses, binds) = session_filter_clause(&filter);
        sql.push_str(&clauses);
        sql.push_str(" ORDER BY created_at DESC");

        let mut q = sqlx::query(&sql);
        for b in &binds {
            q = q.bind(b);
        }

        let rows = q.fetch_all(&self.pool).await.map_err(map_io)?;
        rows.into_iter().map(row_to_session).collect()
    }

    async fn list_sessions_paged(
        &self,
        filter: SessionFilter,
        page: Page,
    ) -> Result<PageResult<SessionMeta>, MemoryError> {
        // Native LIMIT/OFFSET. Cursor is the decimal row offset, matching
        // the trait default's encoding so callers can't tell which impl
        // backs the page. Fetch limit+1 to learn whether a next page
        // exists without a second COUNT query.
        let limit = page.effective_limit() as usize;
        let offset: usize = page
            .cursor
            .as_deref()
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);

        let mut sql = String::from(
            "SELECT id, agent_id, parent_session_id, status, permission_mode, \
             version, created_at, updated_at, deleted_at, extensions, capabilities, \
             current_agent_slug, previous_agent_slug, depth, model \
             FROM sessions WHERE 1=1",
        );
        let (clauses, binds) = session_filter_clause(&filter);
        sql.push_str(&clauses);
        sql.push_str(" ORDER BY created_at DESC LIMIT ? OFFSET ?");

        let mut q = sqlx::query(&sql);
        for b in &binds {
            q = q.bind(b);
        }
        q = q.bind(limit as i64 + 1).bind(offset as i64);

        let rows = q.fetch_all(&self.pool).await.map_err(map_io)?;
        let mut items: Vec<SessionMeta> = rows
            .into_iter()
            .map(row_to_session)
            .collect::<Result<_, _>>()?;
        let next_cursor = if items.len() > limit {
            items.truncate(limit);
            Some((offset + limit).to_string())
        } else {
            None
        };
        Ok(PageResult { items, next_cursor })
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

    async fn append_runtime_reminders(
        &self,
        session: SessionId,
        msg: Message,
        parts: Vec<Part>,
    ) -> Result<Option<(MessageId, Vec<PartId>)>, MemoryError> {
        if parts.is_empty() {
            return Ok(None);
        }

        let mut tx = self.pool.begin().await.map_err(map_io)?;
        let message_id = msg.id;
        sqlx::query(
            r#"INSERT INTO messages (id, session_id, role, seq, created_at, meta)
               VALUES (
                 ?, ?, ?,
                 (SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE session_id = ?),
                 ?, '{}'
               )"#,
        )
        .bind(message_id.to_string())
        .bind(session.to_string())
        .bind(role_str(msg.role))
        .bind(session.to_string())
        .bind(msg.created_at.timestamp_millis())
        .execute(&mut *tx)
        .await
        .map_err(map_io)?;

        let mut inserted = Vec::new();
        for part in parts {
            let Part::RuntimeReminder {
                id,
                reminder_kind,
                ref stable_key,
                projection_epoch,
                ..
            } = part
            else {
                return Err(MemoryError::Io(
                    "append_runtime_reminders received a non-reminder part".into(),
                ));
            };
            let kind = serde_json::to_value(reminder_kind)
                .map_err(|e| MemoryError::Io(format!("encode reminder kind: {e}")))?
                .as_str()
                .ok_or_else(|| MemoryError::Io("reminder kind was not a string".into()))?
                .to_owned();
            let payload = encode_json(&part, "encode runtime reminder")?;
            sqlx::query(
                r#"INSERT INTO parts (id, message_id, seq, kind, payload)
                   VALUES (
                     ?, ?,
                     (SELECT COALESCE(MAX(seq), 0) + 1 FROM parts WHERE message_id = ?),
                     'runtime_reminder', ?
                   )"#,
            )
            .bind(id.to_string())
            .bind(message_id.to_string())
            .bind(message_id.to_string())
            .bind(payload)
            .execute(&mut *tx)
            .await
            .map_err(map_io)?;
            let reservation = sqlx::query(
                r#"INSERT INTO runtime_reminder_deliveries
                   (session_id, reminder_kind, stable_key, projection_epoch, message_id, part_id)
                   VALUES (?, ?, ?, ?, ?, ?)
                   ON CONFLICT(session_id, reminder_kind, stable_key, projection_epoch) DO NOTHING"#,
            )
            .bind(session.to_string())
            .bind(kind)
            .bind(stable_key)
            .bind(i64::from(projection_epoch))
            .bind(message_id.to_string())
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(map_io)?;
            if reservation.rows_affected() == 0 {
                sqlx::query("DELETE FROM parts WHERE id = ?")
                    .bind(id.to_string())
                    .execute(&mut *tx)
                    .await
                    .map_err(map_io)?;
                continue;
            }
            inserted.push(id);
        }

        if inserted.is_empty() {
            sqlx::query("DELETE FROM messages WHERE id = ?")
                .bind(message_id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(map_io)?;
            tx.commit().await.map_err(map_io)?;
            return Ok(None);
        }

        sqlx::query(r#"UPDATE sessions SET updated_at = ? WHERE id = ?"#)
            .bind(now_ms())
            .bind(session.to_string())
            .execute(&mut *tx)
            .await
            .map_err(map_io)?;
        tx.commit().await.map_err(map_io)?;
        Ok(Some((message_id, inserted)))
    }

    async fn append_background_task_settled(
        &self,
        settlement: BackgroundTaskSettlement,
    ) -> Result<Option<(MessageId, Vec<PartId>)>, MemoryError> {
        let message_id = MessageId::new();
        let part_id = PartId::new();
        let stable_key = format!("task:{}", settlement.task_id);
        let kind = serde_json::to_value(ReminderKind::BackgroundTaskSettled)
            .map_err(|e| MemoryError::Io(format!("encode reminder kind: {e}")))?
            .as_str()
            .ok_or_else(|| MemoryError::Io("reminder kind was not a string".into()))?
            .to_owned();

        // Fast path for replay/restart. The unique delivery key remains the
        // linearization point for concurrent settlement workers.
        if let Some(row) = sqlx::query(
            "SELECT message_id, part_id FROM runtime_reminder_deliveries WHERE session_id = ? AND reminder_kind = ? AND stable_key = ? AND projection_epoch = 0",
        )
        .bind(settlement.parent_session_id.to_string())
        .bind(&kind)
        .bind(&stable_key)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_io)?
        {
            // A previous process may have committed the reminder just before
            // it died. Always (re)materialize the outbox row on this
            // idempotent path so a retry restores startup recovery instead
            // of treating the reminder reservation as the whole delivery.
            sqlx::query(
                r#"INSERT INTO background_task_delivery_outbox
                   (parent_session_id, task_id, child_session_id, status, output, cost_usd, scheduled_at)
                   VALUES (?, ?, ?, ?, ?, ?, NULL)
                   ON CONFLICT(parent_session_id, task_id) DO NOTHING"#,
            )
            .bind(settlement.parent_session_id.to_string())
            .bind(&settlement.task_id)
            .bind(settlement.child_session_id.to_string())
            .bind(&settlement.status)
            .bind(&settlement.output)
            .bind(&settlement.cost_usd)
            .execute(&self.pool)
            .await
            .map_err(map_io)?;
            let mid: String = row.try_get("message_id").map_err(map_io)?;
            let pid: String = row.try_get("part_id").map_err(map_io)?;
            let mid = mid
                .parse::<uuid::Uuid>()
                .map(MessageId)
                .map_err(|e| MemoryError::Io(format!("message id: {e}")))?;
            let pid = pid
                .parse::<uuid::Uuid>()
                .map(PartId)
                .map_err(|e| MemoryError::Io(format!("part id: {e}")))?;
            return Ok(Some((mid, vec![pid])));
        }

        let body = format!(
            "Background subagent task {} ({}) settled with status {}.\n{}{}",
            settlement.task_id,
            settlement.child_session_id,
            settlement.status,
            settlement.output,
            settlement
                .cost_usd
                .as_deref()
                .map_or(String::new(), |cost| format!("\nCost: ${cost}"))
        );
        let part = Part::RuntimeReminder {
            id: part_id,
            reminder_kind: ReminderKind::BackgroundTaskSettled,
            stable_key: stable_key.clone(),
            content: body,
            projection_epoch: 0,
        };
        // The reminder, its exactly-once reservation, and its recovery outbox
        // entry are one transaction. A crash therefore exposes either none of
        // them or all of them to restart recovery.
        let mut tx = self.pool.begin().await.map_err(map_io)?;
        sqlx::query(
            r#"INSERT INTO messages (id, session_id, role, seq, created_at, meta)
               VALUES (?, ?, ?,
                 (SELECT COALESCE(MAX(seq), 0) + 1 FROM messages WHERE session_id = ?),
                 ?, '{}')"#,
        )
        .bind(message_id.to_string())
        .bind(settlement.parent_session_id.to_string())
        .bind(role_str(openlet_core::types::message::Role::User))
        .bind(settlement.parent_session_id.to_string())
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&mut *tx)
        .await
        .map_err(map_io)?;
        sqlx::query(
            r#"INSERT INTO parts (id, message_id, seq, kind, payload)
               VALUES (?, ?, 1, 'runtime_reminder', ?)"#,
        )
        .bind(part_id.to_string())
        .bind(message_id.to_string())
        .bind(encode_json(&part, "encode background reminder")?)
        .execute(&mut *tx)
        .await
        .map_err(map_io)?;
        let reservation = sqlx::query(
            r#"INSERT INTO runtime_reminder_deliveries
               (session_id, reminder_kind, stable_key, projection_epoch, message_id, part_id)
               VALUES (?, ?, ?, 0, ?, ?)
               ON CONFLICT(session_id, reminder_kind, stable_key, projection_epoch) DO NOTHING"#,
        )
        .bind(settlement.parent_session_id.to_string())
        .bind(&kind)
        .bind(&stable_key)
        .bind(message_id.to_string())
        .bind(part_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(map_io)?;
        if reservation.rows_affected() == 0 {
            tx.rollback().await.map_err(map_io)?;
            // A concurrent winner committed first. Re-enter through the
            // idempotent path, which also ensures its outbox row exists.
            return self.append_background_task_settled(settlement).await;
        }
        sqlx::query(
            r#"INSERT INTO background_task_delivery_outbox
               (parent_session_id, task_id, child_session_id, status, output, cost_usd, scheduled_at)
               VALUES (?, ?, ?, ?, ?, ?, NULL)
               ON CONFLICT(parent_session_id, task_id) DO NOTHING"#,
        )
        .bind(settlement.parent_session_id.to_string())
        .bind(&settlement.task_id)
        .bind(settlement.child_session_id.to_string())
        .bind(&settlement.status)
        .bind(&settlement.output)
        .bind(&settlement.cost_usd)
        .execute(&mut *tx)
        .await
        .map_err(map_io)?;
        sqlx::query("UPDATE sessions SET updated_at = ? WHERE id = ?")
            .bind(now_ms())
            .bind(settlement.parent_session_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(map_io)?;
        tx.commit().await.map_err(map_io)?;
        Ok(Some((message_id, vec![part_id])))
    }

    async fn claim_background_task_settlements(
        &self,
        parent_session_id: Option<SessionId>,
        task_id: Option<&str>,
    ) -> Result<Vec<ClaimedBackgroundTaskSettlement>, MemoryError> {
        // Each queued/running parent turn renews its own lease. Expiry is
        // therefore a durable crash/panic detector across server processes.
        const LEASE_MS: i64 = 30_000;

        let now = now_ms();
        let mut tx = self.pool.begin().await.map_err(map_io)?;
        let rows = match (parent_session_id, task_id) {
            (Some(parent_session_id), Some(task_id)) => sqlx::query(
                r#"SELECT parent_session_id, task_id, child_session_id, status, output, cost_usd
                   FROM background_task_delivery_outbox
                   WHERE parent_session_id = ? AND task_id = ?
                     AND (delivery_state = 'pending'
                          OR (delivery_state = 'leased' AND lease_expires_at <= ?))
                   ORDER BY rowid"#,
            )
            .bind(parent_session_id.to_string())
            .bind(task_id)
            .bind(now)
            .fetch_all(&mut *tx)
            .await
            .map_err(map_io)?,
            (None, None) => sqlx::query(
                r#"SELECT parent_session_id, task_id, child_session_id, status, output, cost_usd
                   FROM background_task_delivery_outbox
                   WHERE delivery_state = 'pending'
                      OR (delivery_state = 'leased' AND lease_expires_at <= ?)
                   ORDER BY rowid"#,
            )
            .bind(now)
            .fetch_all(&mut *tx)
            .await
            .map_err(map_io)?,
            _ => {
                return Err(MemoryError::Io(
                    "background delivery claim requires both parent session and task id".into(),
                ));
            }
        };

        let mut claimed = Vec::with_capacity(rows.len());
        for row in rows {
            let parent_session_id = row
                .try_get::<String, _>("parent_session_id")
                .map_err(map_io)?
                .parse::<uuid::Uuid>()
                .map(SessionId)
                .map_err(|e| MemoryError::Io(e.to_string()))?;
            let task_id: String = row.try_get("task_id").map_err(map_io)?;
            let lease_id = uuid::Uuid::new_v4().to_string();
            let updated = sqlx::query(
                r#"UPDATE background_task_delivery_outbox
                   SET delivery_state = 'leased', lease_id = ?,
                       lease_expires_at = ?, delivery_attempts = delivery_attempts + 1
                   WHERE parent_session_id = ? AND task_id = ?
                     AND (delivery_state = 'pending'
                          OR (delivery_state = 'leased' AND lease_expires_at <= ?))"#,
            )
            .bind(&lease_id)
            .bind(now + LEASE_MS)
            .bind(parent_session_id.to_string())
            .bind(&task_id)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(map_io)?;
            if updated.rows_affected() == 0 {
                continue;
            }

            let child_session_id = row
                .try_get::<String, _>("child_session_id")
                .map_err(map_io)?
                .parse::<uuid::Uuid>()
                .map(SessionId)
                .map_err(|e| MemoryError::Io(e.to_string()))?;
            claimed.push(ClaimedBackgroundTaskSettlement {
                settlement: BackgroundTaskSettlement {
                    parent_session_id,
                    task_id,
                    child_session_id,
                    status: row.try_get("status").map_err(map_io)?,
                    output: row.try_get("output").map_err(map_io)?,
                    cost_usd: row.try_get("cost_usd").map_err(map_io)?,
                },
                lease_id,
            });
        }
        tx.commit().await.map_err(map_io)?;
        Ok(claimed)
    }

    async fn acknowledge_background_task_settlement(
        &self,
        parent_session_id: SessionId,
        task_id: &str,
        lease_id: &str,
    ) -> Result<(), MemoryError> {
        let updated = sqlx::query(
            r#"UPDATE background_task_delivery_outbox
               SET delivery_state = 'delivered', delivered_at = ?,
                   lease_id = NULL, lease_expires_at = NULL
               WHERE parent_session_id = ? AND task_id = ?
                 AND delivery_state = 'leased' AND lease_id = ?"#,
        )
        .bind(now_ms())
        .bind(parent_session_id.to_string())
        .bind(task_id)
        .bind(lease_id)
        .execute(&self.pool)
        .await
        .map_err(map_io)?;
        if updated.rows_affected() == 0 {
            return Err(MemoryError::Io(
                "background delivery lease was lost before acknowledgement".into(),
            ));
        }
        Ok(())
    }

    async fn release_background_task_settlement(
        &self,
        parent_session_id: SessionId,
        task_id: &str,
        lease_id: &str,
    ) -> Result<(), MemoryError> {
        let released = sqlx::query(
            r#"UPDATE background_task_delivery_outbox
               SET delivery_state = 'pending', lease_id = NULL, lease_expires_at = NULL
               WHERE parent_session_id = ? AND task_id = ?
                 AND delivery_state = 'leased' AND lease_id = ?"#,
        )
        .bind(parent_session_id.to_string())
        .bind(task_id)
        .bind(lease_id)
        .execute(&self.pool)
        .await
        .map_err(map_io)?;
        if released.rows_affected() == 0 {
            return Err(MemoryError::Io(
                "background delivery lease was lost before release".into(),
            ));
        }
        Ok(())
    }

    async fn renew_background_task_settlement_lease(
        &self,
        parent_session_id: SessionId,
        task_id: &str,
        lease_id: &str,
    ) -> Result<(), MemoryError> {
        const LEASE_MS: i64 = 30_000;
        let renewed = sqlx::query(
            "UPDATE background_task_delivery_outbox SET lease_expires_at = ? WHERE parent_session_id = ? AND task_id = ? AND delivery_state = 'leased' AND lease_id = ?",
        )
        .bind(now_ms() + LEASE_MS)
        .bind(parent_session_id.to_string())
        .bind(task_id)
        .bind(lease_id)
        .execute(&self.pool)
        .await
        .map_err(map_io)?;
        if renewed.rows_affected() == 0 {
            return Err(MemoryError::Io(
                "background delivery lease was lost before renewal".into(),
            ));
        }
        Ok(())
    }

    async fn upsert_part(
        &self,
        msg: MessageId,
        part_id: PartId,
        part: Part,
    ) -> Result<(), MemoryError> {
        let kind = part_kind(&part);
        let payload = encode_json(&part, "encode part")?;

        // Single atomic INSERT with the seq as an in-statement subquery —
        // same pattern as append_message/append_part (B/I2). The previous
        // form computed next_seq in a SEPARATE `SELECT ... fetch_one` and
        // then INSERTed, which raced: two concurrent upserts of DISTINCT
        // fresh part_ids on the same message could both read the same
        // next_seq and the second would violate UNIQUE(message_id, seq)
        // (the ON CONFLICT targets `id`, not `seq`, so it wouldn't catch
        // the seq collision). Folding the MAX(seq) into the INSERT makes
        // SQLite serialize the read+write under the writer lock. The
        // subquery is only materialized for a NEW row; on ON CONFLICT(id)
        // the computed seq is discarded and only kind/payload are updated,
        // so an existing part keeps its original seq.
        sqlx::query(
            r#"INSERT INTO parts (id, message_id, seq, kind, payload)
               VALUES (
                 ?, ?,
                 (SELECT COALESCE(MAX(seq), -1) + 1 FROM parts WHERE message_id = ?),
                 ?, ?
               )
               ON CONFLICT(id) DO UPDATE SET
                   kind = excluded.kind,
                   payload = excluded.payload"#,
        )
        .bind(part_id.to_string())
        .bind(msg.to_string())
        .bind(msg.to_string())
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

    async fn list_messages_paged(
        &self,
        session: SessionId,
        page: Page,
    ) -> Result<PageResult<Message>, MemoryError> {
        let limit = page.effective_limit() as usize;
        let offset: usize = page
            .cursor
            .as_deref()
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);

        let rows = sqlx::query(
            r#"SELECT id, session_id, role, created_at FROM messages
               WHERE session_id = ? ORDER BY seq ASC LIMIT ? OFFSET ?"#,
        )
        .bind(session.to_string())
        .bind(limit as i64 + 1)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(map_io)?;

        let mut items: Vec<Message> = rows
            .into_iter()
            .map(row_to_message)
            .collect::<Result<_, _>>()?;
        let next_cursor = if items.len() > limit {
            items.truncate(limit);
            Some((offset + limit).to_string())
        } else {
            None
        };
        Ok(PageResult { items, next_cursor })
    }

    async fn record_read(&self, session: SessionId, path: PathBuf) -> Result<(), MemoryError> {
        let path_str = path.to_string_lossy().to_string();
        // Legacy path-only record: leave fingerprint/scope untouched on
        // conflict so a bare read never erases a richer prior observation.
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

    async fn record_observation(
        &self,
        obs: openlet_core::adapters::memory_store::ReadObservation,
    ) -> Result<(), MemoryError> {
        let path_str = obs.path.to_string_lossy().to_string();
        // Atomic upsert by (session_id, path). The fingerprint + scope are
        // always overwritten with the latest observation so a re-read of a
        // changed file records its new content hash.
        sqlx::query(
            r#"INSERT INTO session_reads (session_id, path, read_at, fingerprint, scope)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(session_id, path) DO UPDATE SET
                   read_at = excluded.read_at,
                   fingerprint = excluded.fingerprint,
                   scope = excluded.scope"#,
        )
        .bind(obs.session_id.to_string())
        .bind(path_str)
        .bind(now_ms())
        .bind(obs.fingerprint.as_deref())
        .bind(obs.scope.as_str())
        .execute(&self.pool)
        .await
        .map_err(map_io)?;
        Ok(())
    }

    async fn list_observations(
        &self,
        session: SessionId,
    ) -> Result<Vec<openlet_core::adapters::memory_store::ReadObservation>, MemoryError> {
        use openlet_core::adapters::memory_store::{ReadObservation, ReadScope};
        let rows = sqlx::query(
            r#"SELECT path, fingerprint, scope FROM session_reads
               WHERE session_id = ? ORDER BY read_at ASC"#,
        )
        .bind(session.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(map_io)?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let path: String = row.try_get("path").map_err(map_io)?;
            let fingerprint: Option<String> = row.try_get("fingerprint").map_err(map_io)?;
            let scope: Option<String> = row.try_get("scope").map_err(map_io)?;
            out.push(ReadObservation {
                session_id: session,
                path: PathBuf::from(path),
                fingerprint,
                scope: scope
                    .as_deref()
                    .map(ReadScope::from_label)
                    .unwrap_or(ReadScope::Full),
            });
        }
        Ok(out)
    }

    async fn list_parts(
        &self,
        session: SessionId,
        msg: MessageId,
    ) -> Result<Vec<Part>, MemoryError> {
        let rows = sqlx::query(
            r#"SELECT p.payload FROM parts p
               INNER JOIN messages m ON m.id = p.message_id
               WHERE p.message_id = ? AND m.session_id = ?
               ORDER BY p.seq ASC"#,
        )
        .bind(msg.to_string())
        .bind(session.to_string())
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
