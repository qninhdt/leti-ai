//! Per-session JSONL mirror — append-only log of every event for replay/audit.
//!
//! Path: `<root>/<session_id>.jsonl`. Writes are line-buffered + flushed.
//! Secrets are redacted before serialization (regex plus
//! a key-name allowlist). Files rotate at 64MB (`.jsonl` -> `.jsonl.1`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use leti_core::types::event::AgentEvent;
use leti_core::types::session::SessionId;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::Mutex;

use super::redactor::SecretRedactor;

const ROTATE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct SessionLogger {
    root: PathBuf,
    redactor: Arc<SecretRedactor>,
    locks: Arc<dashmap::DashMap<SessionId, Arc<Mutex<()>>>>,
}

impl std::fmt::Debug for SessionLogger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionLogger")
            .field("root", &self.root)
            .finish()
    }
}

impl SessionLogger {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            redactor: Arc::new(SecretRedactor::default()),
            locks: Arc::new(dashmap::DashMap::new()),
        }
    }

    fn lock_for(&self, session: SessionId) -> Arc<Mutex<()>> {
        self.locks
            .entry(session)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn path_for(&self, session: SessionId) -> PathBuf {
        self.root.join(format!("{session}.jsonl"))
    }

    pub async fn append(&self, session: SessionId, ev: &AgentEvent) -> std::io::Result<()> {
        tokio::fs::create_dir_all(&self.root).await?;

        let mut value = serde_json::to_value(ev)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        self.redactor.redact_in_place(&mut value);

        let envelope = serde_json::json!({
            "ts": Utc::now().to_rfc3339(),
            "event": value,
        });
        let mut line = serde_json::to_string(&envelope)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        line.push('\n');

        let lock = self.lock_for(session);
        let _g = lock.lock().await;

        let path = self.path_for(session);
        rotate_if_needed(&path).await?;

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;
        let mut w = BufWriter::new(file);
        w.write_all(line.as_bytes()).await?;
        w.flush().await?;
        // `flush()` only drains the user buffer to the kernel page
        // cache; a process crash within the writeback window loses the
        // event. The companion `events` SQLite table is fsynced via
        // `synchronous=NORMAL`, so the JSONL audit log was the *less*
        // durable of the two stores. `sync_data()` brings it to parity.
        // Errors here propagate so callers can fall back to SQLite-only
        // replay if disk is failing.
        let inner = w.into_inner();
        inner.sync_data().await?;
        Ok(())
    }

    /// Evict the per-session lock entry. Called when a session reaches a
    /// terminal status so the lock map doesn't grow linearly with the
    /// number of distinct SessionIds the process ever observed. Idempotent.
    pub fn evict_session(&self, session: SessionId) {
        self.locks.remove(&session);
    }
}

async fn rotate_if_needed(path: &Path) -> std::io::Result<()> {
    let meta = match tokio::fs::metadata(path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if meta.len() < ROTATE_BYTES {
        return Ok(());
    }
    let mut rotated = path.to_path_buf();
    rotated.set_extension("jsonl.1");
    let _ = tokio::fs::remove_file(&rotated).await;
    tokio::fs::rename(path, rotated).await
}
