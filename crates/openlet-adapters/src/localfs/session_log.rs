//! Per-session JSONL mirror — append-only log of every event for replay/audit.
//!
//! Path: `<root>/<session_id>.jsonl`. Writes are line-buffered + flushed.
//! Secrets are redacted before serialization (regex per amendment §M plus
//! a key-name allowlist). Files rotate at 64MB (`.jsonl` -> `.jsonl.1`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::session::SessionId;
use regex::Regex;
use serde_json::Value;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::sync::Mutex;

const ROTATE_BYTES: u64 = 64 * 1024 * 1024;
const SENSITIVE_KEYS: &[&str] = &[
    "api_key",
    "apikey",
    "authorization",
    "x-api-key",
    "password",
    "secret",
    "token",
    "access_token",
    "refresh_token",
];

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
        Ok(())
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

#[derive(Debug)]
pub struct SecretRedactor {
    bearer: Regex,
    sk_token: Regex,
    sensitive: Vec<String>,
}

impl Default for SecretRedactor {
    fn default() -> Self {
        Self {
            bearer: Regex::new(r"(?i)bearer\s+[A-Za-z0-9\-_.=]+").expect("bearer regex"),
            sk_token: Regex::new(r"sk-[A-Za-z0-9_\-]{16,}").expect("sk token regex"),
            sensitive: SENSITIVE_KEYS.iter().map(|s| s.to_lowercase()).collect(),
        }
    }
}

impl SecretRedactor {
    fn is_sensitive_key(&self, k: &str) -> bool {
        let lk = k.to_lowercase();
        self.sensitive.iter().any(|s| lk == *s || lk.contains(s))
    }

    pub fn redact_in_place(&self, v: &mut Value) {
        match v {
            Value::Object(map) => {
                for (k, val) in map.iter_mut() {
                    if self.is_sensitive_key(k) {
                        *val = Value::String("<redacted>".into());
                    } else {
                        self.redact_in_place(val);
                    }
                }
            }
            Value::Array(arr) => {
                for item in arr.iter_mut() {
                    self.redact_in_place(item);
                }
            }
            Value::String(s) => {
                let redacted = self.bearer.replace_all(s, "<redacted>");
                let redacted = self.sk_token.replace_all(&redacted, "<redacted>");
                *v = Value::String(redacted.into_owned());
            }
            _ => {}
        }
    }
}
