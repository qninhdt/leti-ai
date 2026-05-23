//! Append-only writer for the `events` table. Returns the autoincrement
//! `id` so phase-05's SSE channel can use it as Last-Event-ID.

use chrono::Utc;
use sqlx::SqlitePool;

use openlet_core::error::EventError;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::session::SessionId;

#[derive(Debug, Clone)]
pub struct SqliteEventRepo {
    pool: SqlitePool,
}

impl SqliteEventRepo {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn append(
        &self,
        session_id: Option<SessionId>,
        ev: &AgentEvent,
    ) -> Result<i64, EventError> {
        let kind = event_kind(ev);
        let payload = serde_json::to_string(ev)
            .map_err(|e| EventError::Io(format!("encode event: {e}")))?;
        let now = Utc::now().timestamp_millis();

        let id: i64 = sqlx::query_scalar(
            r#"INSERT INTO events (session_id, kind, payload, created_at)
               VALUES (?, ?, ?, ?) RETURNING id"#,
        )
        .bind(session_id.map(|s| s.to_string()))
        .bind(kind)
        .bind(&payload)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| EventError::Io(e.to_string()))?;

        Ok(id)
    }

    pub async fn list_since(
        &self,
        session_id: SessionId,
        after_id: i64,
    ) -> Result<Vec<(i64, AgentEvent)>, EventError> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            r#"SELECT id, payload FROM events
               WHERE session_id = ? AND id > ?
               ORDER BY id ASC"#,
        )
        .bind(session_id.to_string())
        .bind(after_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| EventError::Io(e.to_string()))?;

        rows.into_iter()
            .map(|(id, p)| {
                serde_json::from_str::<AgentEvent>(&p)
                    .map(|ev| (id, ev))
                    .map_err(|e| EventError::Io(format!("decode event: {e}")))
            })
            .collect()
    }
}

fn event_kind(ev: &AgentEvent) -> &'static str {
    match ev {
        AgentEvent::SessionStatus { .. } => "session.status",
        AgentEvent::MessageCreated { .. } => "message.created",
        AgentEvent::PartCreated { .. } => "part.created",
        AgentEvent::PartDelta { .. } => "part.delta",
        AgentEvent::PartUpdated { .. } => "part.updated",
        AgentEvent::StepFinished { .. } => "step.finished",
        AgentEvent::PermissionAsked { .. } => "permission.asked",
        AgentEvent::PermissionResolved { .. } => "permission.resolved",
        AgentEvent::Error { .. } => "error",
        AgentEvent::Heartbeat => "heartbeat",
    }
}
