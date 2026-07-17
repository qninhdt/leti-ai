//! In-memory `ArtifactStore` for tests. Backed by a `HashMap<String, Bytes>`
//! keyed on `(session_id, key)` so per-session listing works.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use bytes::Bytes;
use leti_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use leti_core::error::ArtifactError;
use leti_core::types::session::SessionId;

type ArtifactKey = (SessionId, String);
type ArtifactValue = (Bytes, Option<String>);

#[derive(Default)]
pub struct MemArtifactStore {
    map: Mutex<HashMap<ArtifactKey, ArtifactValue>>,
}

impl MemArtifactStore {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn count(&self) -> usize {
        self.map.lock().unwrap().len()
    }
}

#[async_trait]
impl ArtifactStore for MemArtifactStore {
    async fn put(
        &self,
        session: SessionId,
        key: &str,
        bytes: Bytes,
    ) -> Result<ArtifactRef, ArtifactError> {
        let size = bytes.len() as u64;
        self.map
            .lock()
            .unwrap()
            .insert((session, key.to_string()), (bytes, None));
        Ok(ArtifactRef {
            session_id: session,
            key: key.to_string(),
            size,
            mime: None,
        })
    }

    async fn get(&self, r: &ArtifactRef) -> Result<Bytes, ArtifactError> {
        self.map
            .lock()
            .unwrap()
            .get(&(r.session_id, r.key.clone()))
            .map(|(b, _)| b.clone())
            .ok_or_else(|| ArtifactError::NotFound(r.key.clone()))
    }

    async fn list(&self, session: SessionId) -> Result<Vec<ArtifactRef>, ArtifactError> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .iter()
            .filter(|((s, _), _)| *s == session)
            .map(|((s, k), (b, mime))| ArtifactRef {
                session_id: *s,
                key: k.clone(),
                size: b.len() as u64,
                mime: mime.clone(),
            })
            .collect())
    }
}
