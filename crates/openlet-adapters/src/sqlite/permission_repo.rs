//! Permission decision persistence — used by `ConfigPermissionMgr` to honor
//! "always allow" / "always deny" choices across turns.

use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

use openlet_core::error::PermissionError;
use openlet_core::types::permission::AskId;
use openlet_core::types::session::SessionId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistedDecision {
    Allow,
    Deny,
    Always,
    Never,
}

impl PersistedDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Always => "always",
            Self::Never => "never",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PermissionRecord {
    pub session_id: SessionId,
    pub ask_id: AskId,
    pub permission: String,
    pub decision: PersistedDecision,
}

#[derive(Debug, Clone)]
pub struct SqlitePermissionRepo {
    pool: SqlitePool,
}

/// Map a `sqlx::Error` to `PermissionError::Io` carrying its textual form.
fn map_io(e: sqlx::Error) -> PermissionError {
    PermissionError::Io(e.to_string())
}

impl SqlitePermissionRepo {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn record(&self, rec: &PermissionRecord) -> Result<(), PermissionError> {
        let row_id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            r#"INSERT INTO permission_decisions
                 (id, session_id, ask_id, permission, decision, created_at)
               VALUES (?, ?, ?, ?, ?, ?)
               ON CONFLICT(session_id, ask_id) DO UPDATE SET
                 permission = excluded.permission,
                 decision   = excluded.decision,
                 created_at = excluded.created_at"#,
        )
        .bind(row_id)
        .bind(rec.session_id.to_string())
        .bind(rec.ask_id.0.to_string())
        .bind(&rec.permission)
        .bind(rec.decision.as_str())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_io)?;
        Ok(())
    }

    pub async fn list_for_session(
        &self,
        session_id: SessionId,
    ) -> Result<Vec<PermissionRecord>, PermissionError> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            r#"SELECT ask_id, permission, decision FROM permission_decisions
               WHERE session_id = ?"#,
        )
        .bind(session_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(map_io)?;

        rows.into_iter()
            .map(|(ask, perm, dec)| {
                let ask_uuid = Uuid::parse_str(&ask)
                    .map_err(|e| PermissionError::Io(format!("ask uuid: {e}")))?;
                let decision = PersistedDecision::parse(&dec)
                    .ok_or_else(|| PermissionError::Io(format!("unknown decision {dec}")))?;
                Ok(PermissionRecord {
                    session_id,
                    ask_id: AskId(ask_uuid),
                    permission: perm,
                    decision,
                })
            })
            .collect()
    }
}
