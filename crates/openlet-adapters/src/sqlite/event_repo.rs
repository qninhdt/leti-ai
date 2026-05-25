//! Append-only writer for the `events` table. Returns the autoincrement
//! `id` so phase-05's SSE channel can use it as Last-Event-ID.

use chrono::Utc;
use sqlx::SqlitePool;

use openlet_core::error::EventError;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::session::SessionId;

/// Bounded replay window. Anything older than `(MAX(id) - REPLAY_WINDOW)`
/// is rejected with `EventError::CursorTooFarBehind` so a malicious or
/// long-disconnected client cannot OOM the server with a giant replay.
const REPLAY_WINDOW: i64 = 100_000;
/// Per-page replay cap. The HTTP layer surfaces this as the maximum
/// rows returned per `Last-Event-ID` reconnect.
const REPLAY_PAGE_LIMIT: i64 = 1000;

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
        let payload =
            serde_json::to_string(ev).map_err(|e| EventError::Io(format!("encode event: {e}")))?;
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

    /// Reject `after_id` more than `REPLAY_WINDOW` rows behind tip so a
    /// `?after=0` request can't load the entire session history into
    /// memory. Returns `EventError::CursorTooFarBehind` for the HTTP
    /// layer to map to 409 with a reset hint.
    async fn check_cursor_in_window(&self, after_id: i64) -> Result<(), EventError> {
        let max_id: Option<i64> = sqlx::query_scalar(r#"SELECT MAX(id) FROM events"#)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| EventError::Io(e.to_string()))?;
        let Some(max_id) = max_id else { return Ok(()) };
        if after_id < max_id.saturating_sub(REPLAY_WINDOW) {
            return Err(EventError::CursorTooFarBehind {
                requested: after_id,
                tip: max_id,
                window: REPLAY_WINDOW,
            });
        }
        Ok(())
    }

    pub async fn list_since(
        &self,
        session_id: SessionId,
        after_id: i64,
    ) -> Result<Vec<(i64, AgentEvent)>, EventError> {
        self.check_cursor_in_window(after_id).await?;
        let rows: Vec<(i64, String)> = sqlx::query_as(
            r#"SELECT id, payload FROM events
               WHERE session_id = ? AND id > ?
               ORDER BY id ASC
               LIMIT ?"#,
        )
        .bind(session_id.to_string())
        .bind(after_id)
        .bind(REPLAY_PAGE_LIMIT)
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

    /// Global replay (no session filter). Used by the global SSE channel
    /// when `Last-Event-ID` is present without a `?session=` query.
    /// Same window + page-limit semantics as `list_since`.
    pub async fn list_since_global(
        &self,
        after_id: i64,
    ) -> Result<Vec<(i64, AgentEvent)>, EventError> {
        self.check_cursor_in_window(after_id).await?;
        let rows: Vec<(i64, String)> = sqlx::query_as(
            r#"SELECT id, payload FROM events
               WHERE id > ?
               ORDER BY id ASC
               LIMIT ?"#,
        )
        .bind(after_id)
        .bind(REPLAY_PAGE_LIMIT)
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
        AgentEvent::PluginError { .. } => "plugin.error",
        AgentEvent::QuestionRequested { .. } => "question.requested",
        AgentEvent::PlanModeEntered { .. } => "plan_mode.entered",
        AgentEvent::PlanModeExited { .. } => "plan_mode.exited",
        AgentEvent::AttachmentAccepted { .. } => "attachment.accepted",
        AgentEvent::Heartbeat => "heartbeat",
    }
}
