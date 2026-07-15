//! Append-only writer for the `events` table. Returns the autoincrement
//! `id` so the SSE channel can use it as Last-Event-ID.

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

    /// Highest persisted event id, or 0 when the table is empty. Used by
    /// `BroadcastBus` to seed its monotonic event-id counter at boot so a
    /// pre-assigned id never collides with a row that survived a restart
    /// (the `events` table is durable; a counter starting at 0 each boot
    /// would re-issue ids 1.. and hit a `UNIQUE` PK violation).
    pub async fn max_event_id(&self) -> Result<i64, EventError> {
        let max: i64 = sqlx::query_scalar(r#"SELECT COALESCE(MAX(id), 0) FROM events"#)
            .fetch_one(&self.pool)
            .await
            .map_err(map_io)?;
        Ok(max)
    }

    /// Append an event with a caller-assigned `event_id` (the id is
    /// allocated by `BroadcastBus` from its monotonic counter rather than
    /// by SQLite `AUTOINCREMENT`). The PK is supplied explicitly so the
    /// broadcast layer owns id assignment + ordering. A duplicate id
    /// surfaces as the underlying `UNIQUE` violation mapped to
    /// `EventError::Io` — it signals a counter-seed bug, never normal
    /// operation.
    pub async fn append_with_id(
        &self,
        event_id: i64,
        session_id: Option<SessionId>,
        ev: &AgentEvent,
    ) -> Result<(), EventError> {
        let kind = ev.kind();
        let payload =
            serde_json::to_string(ev).map_err(|e| EventError::Io(format!("encode event: {e}")))?;
        let now = Utc::now().timestamp_millis();

        sqlx::query(
            r#"INSERT INTO events (id, session_id, kind, payload, created_at)
               VALUES (?, ?, ?, ?, ?)"#,
        )
        .bind(event_id)
        .bind(session_id.map(|s| s.to_string()))
        .bind(kind)
        .bind(&payload)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_io)?;

        Ok(())
    }

    /// Reject `after_id` more than `REPLAY_WINDOW` rows behind tip so a
    /// `?after=0` request can't load the entire session history into
    /// memory. Returns `EventError::CursorTooFarBehind` for the HTTP
    /// layer to map to 409 with a reset hint.
    async fn check_cursor_in_window(&self, after_id: i64) -> Result<(), EventError> {
        let max_id: Option<i64> = sqlx::query_scalar(r#"SELECT MAX(id) FROM events"#)
            .fetch_one(&self.pool)
            .await
            .map_err(map_io)?;
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
        let tip: Option<i64> = sqlx::query_scalar("SELECT MAX(id) FROM events")
            .fetch_one(&self.pool)
            .await
            .map_err(map_io)?;
        let Some(tip) = tip else {
            return Ok(Vec::new());
        };
        let mut cursor = after_id;
        let mut rows = Vec::new();
        loop {
            let page: Vec<(i64, String)> = sqlx::query_as(
                r#"SELECT id, payload FROM events
                   WHERE session_id = ? AND id > ? AND id <= ?
                   ORDER BY id ASC LIMIT ?"#,
            )
            .bind(session_id.to_string())
            .bind(cursor)
            .bind(tip)
            .bind(REPLAY_PAGE_LIMIT)
            .fetch_all(&self.pool)
            .await
            .map_err(map_io)?;
            let count = page.len();
            if let Some((id, _)) = page.last() {
                cursor = *id;
            }
            rows.extend(page);
            if count < REPLAY_PAGE_LIMIT as usize {
                break;
            }
        }
        decode_rows(rows)
    }

    /// Global replay (no session filter). Used by the global SSE channel
    /// when `Last-Event-ID` is present without a `?session=` query.
    /// Same window + page-limit semantics as `list_since`.
    pub async fn list_since_global(
        &self,
        after_id: i64,
    ) -> Result<Vec<(i64, AgentEvent)>, EventError> {
        self.check_cursor_in_window(after_id).await?;
        let tip: Option<i64> = sqlx::query_scalar("SELECT MAX(id) FROM events")
            .fetch_one(&self.pool)
            .await
            .map_err(map_io)?;
        let Some(tip) = tip else {
            return Ok(Vec::new());
        };
        let mut cursor = after_id;
        let mut rows = Vec::new();
        loop {
            let page: Vec<(i64, String)> = sqlx::query_as(
                r#"SELECT id, payload FROM events
                   WHERE id > ? AND id <= ?
                   ORDER BY id ASC LIMIT ?"#,
            )
            .bind(cursor)
            .bind(tip)
            .bind(REPLAY_PAGE_LIMIT)
            .fetch_all(&self.pool)
            .await
            .map_err(map_io)?;
            let count = page.len();
            if let Some((id, _)) = page.last() {
                cursor = *id;
            }
            rows.extend(page);
            if count < REPLAY_PAGE_LIMIT as usize {
                break;
            }
        }
        decode_rows(rows)
    }
}

/// Map a `sqlx::Error` to `EventError::Io` carrying its textual form.
fn map_io(e: sqlx::Error) -> EventError {
    EventError::Io(e.to_string())
}

/// Decode `(id, payload)` rows into `(id, AgentEvent)`, surfacing a
/// contextual `EventError::Io` on the first malformed payload. Shared by
/// the per-session and global replay paths.
fn decode_rows(rows: Vec<(i64, String)>) -> Result<Vec<(i64, AgentEvent)>, EventError> {
    rows.into_iter()
        .map(|(id, p)| {
            serde_json::from_str::<AgentEvent>(&p)
                .map(|ev| (id, ev))
                .map_err(|e| EventError::Io(format!("decode event: {e}")))
        })
        .collect()
}
