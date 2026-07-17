//! `LocalFsArtifactStore` — `ArtifactStore` impl writing under
//! `<root>/<session_id>/<sha256(key).hex>` with metadata in SQLite.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use uuid::Uuid;

use leti_core::adapters::artifact_store::{ArtifactRef, ArtifactStore, ByteStream};
use leti_core::error::ArtifactError;
use leti_core::types::session::SessionId;

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
        // Crash-safe atomic write: stage into a tempfile in the same dir,
        // sync it, then rename(2) over the target and sync the parent
        // directory. A process crash or power loss cannot leave a torn
        // `todos.json` (or another artifact) on disk.
        // tempfile is sync, so offload to the blocking pool.
        let path_clone = path.clone();
        let dir_clone = dir.clone();
        let body = bytes.clone();
        tokio::task::spawn_blocking(move || -> Result<(), std::io::Error> {
            use std::io::Write as _;

            let mut tmp = tempfile::NamedTempFile::new_in(&dir_clone)?;
            tmp.write_all(&body)?;
            tmp.as_file().sync_all()?;
            tmp.persist(&path_clone).map_err(|e| e.error)?;
            // POSIX directory fsync makes the rename durable. Windows does
            // not support syncing directory handles in the same way, while
            // `sync_all` above still flushes the replacement file.
            #[cfg(unix)]
            std::fs::File::open(&dir_clone)?.sync_all()?;
            Ok(())
        })
        .await
        .map_err(|e| ArtifactError::Io(format!("atomic artifact write join: {e}")))?
        .map_err(map_io)?;

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

    async fn get_stream(&self, r: &ArtifactRef) -> Result<ByteStream, ArtifactError> {
        use futures::StreamExt;
        use tokio_util::io::ReaderStream;

        validate_key(&r.key)?;
        let path = self.key_path(r.session_id, &r.key);
        let file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => ArtifactError::NotFound(r.key.clone()),
                _ => ArtifactError::Io(e.to_string()),
            })?;
        // Stream the file in chunks rather than buffering the whole body —
        // the reference impl for cloud streaming stores.
        let stream =
            ReaderStream::new(file).map(|res| res.map_err(|e| ArtifactError::Io(e.to_string())));
        Ok(stream.boxed())
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
