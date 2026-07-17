//! Per-session record of paths the model has read this session.
//!
//! Used by `write` and `edit` tools to enforce read-before-write
//! (Anthropic str_replace pattern — relies on a
//! diff-or-prior-read safety bar).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

/// Cloneable handle to a per-session set of canonicalized paths the
/// runtime has observed via the `read` (or `glob`) tools.
#[derive(Debug, Clone, Default)]
pub struct ReadHistory {
    inner: Arc<Mutex<HashSet<PathBuf>>>,
}

impl ReadHistory {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record `path` as having been read. Caller is responsible for
    /// canonicalization — pass the resolved workspace-relative path.
    pub async fn record(&self, path: PathBuf) {
        let mut g = self.inner.lock().await;
        g.insert(path);
    }

    /// Test whether `path` was previously recorded.
    pub async fn contains(&self, path: &Path) -> bool {
        let g = self.inner.lock().await;
        g.contains(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn record_then_contains() {
        let h = ReadHistory::new();
        let p: PathBuf = "/tmp/foo".into();
        assert!(!h.contains(&p).await);
        h.record(p.clone()).await;
        assert!(h.contains(&p).await);
    }

    #[tokio::test]
    async fn shared_handle_sees_records() {
        let h = ReadHistory::new();
        let h2 = h.clone();
        let p: PathBuf = "/tmp/bar".into();
        h.record(p.clone()).await;
        assert!(h2.contains(&p).await);
    }
}
