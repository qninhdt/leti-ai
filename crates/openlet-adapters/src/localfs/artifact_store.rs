//! `LocalFsArtifactStore` — `ArtifactStore` impl writing under
//! `<root>/<session_id>/<sha256(key).hex>` with metadata in SQLite.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use openlet_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use openlet_core::error::ArtifactError;
use openlet_core::types::session::SessionId;

#[derive(Debug, Clone)]
pub struct LocalFsArtifactStore {
    root: PathBuf,
    pool: SqlitePool,
}

impl LocalFsArtifactStore {
    #[must_use]
    pub fn new(root: PathBuf, pool: SqlitePool) -> Self {
        Self { root, pool }
    }

    fn session_dir(&self, session: SessionId) -> PathBuf {
        self.root.join(session.to_string())
    }

    fn key_path(&self, session: SessionId, key: &str) -> PathBuf {
        let mut hasher = Sha256::new();
        hasher.update(key.as_bytes());
        let hex = hex::encode(hasher.finalize());
        self.session_dir(session).join(hex)
    }
}

fn map_io(e: std::io::Error) -> ArtifactError {
    ArtifactError::Io(e.to_string())
}

fn map_db(e: sqlx::Error) -> ArtifactError {
    ArtifactError::Io(e.to_string())
}

fn validate_key(key: &str) -> Result<(), ArtifactError> {
    if key.is_empty() {
        return Err(ArtifactError::Io("artifact key empty".into()));
    }
    if key.contains("..") || key.starts_with('/') || key.starts_with('\\') {
        return Err(ArtifactError::Io(format!("rejected unsafe key: {key}")));
    }
    Ok(())
}

#[async_trait]
impl ArtifactStore for LocalFsArtifactStore {
    async fn put(
        &self,
        session: SessionId,
        key: &str,
        bytes: Bytes,
    ) -> Result<ArtifactRef, ArtifactError> {
        validate_key(key)?;

        let dir = self.session_dir(session);
        tokio::fs::create_dir_all(&dir).await.map_err(map_io)?;
        let path = self.key_path(session, key);
        tokio::fs::write(&path, &bytes).await.map_err(map_io)?;

        let size = bytes.len() as i64;
        let rel: PathBuf =
            Path::new(&session.to_string()).join(path.file_name().expect("artifact filename"));
        let rel_str = rel.to_string_lossy().to_string();
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp_millis();

        sqlx::query(
            r#"INSERT INTO artifacts
                 (id, session_id, key, bytes_path, size_bytes, mime, created_at)
               VALUES (?, ?, ?, ?, ?, NULL, ?)
               ON CONFLICT(session_id, key) DO UPDATE SET
                 bytes_path = excluded.bytes_path,
                 size_bytes = excluded.size_bytes,
                 created_at = excluded.created_at"#,
        )
        .bind(&id)
        .bind(session.to_string())
        .bind(key)
        .bind(&rel_str)
        .bind(size)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(map_db)?;

        Ok(ArtifactRef {
            session_id: session,
            key: key.to_string(),
            size: size as u64,
            mime: None,
        })
    }

    async fn get(&self, r: &ArtifactRef) -> Result<Bytes, ArtifactError> {
        validate_key(&r.key)?;
        let path = self.key_path(r.session_id, &r.key);
        let bytes = tokio::fs::read(&path).await.map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ArtifactError::NotFound(r.key.clone()),
            _ => ArtifactError::Io(e.to_string()),
        })?;
        Ok(Bytes::from(bytes))
    }

    async fn list(&self, session: SessionId) -> Result<Vec<ArtifactRef>, ArtifactError> {
        let rows = sqlx::query(
            r#"SELECT key, size_bytes, mime FROM artifacts
               WHERE session_id = ? ORDER BY created_at ASC"#,
        )
        .bind(session.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(map_db)?;

        rows.into_iter()
            .map(|row| {
                let key: String = row.try_get("key").map_err(map_db)?;
                let size: i64 = row.try_get("size_bytes").map_err(map_db)?;
                let mime: Option<String> = row.try_get("mime").map_err(map_db)?;
                Ok(ArtifactRef {
                    session_id: session,
                    key,
                    size: size as u64,
                    mime,
                })
            })
            .collect()
    }
}
