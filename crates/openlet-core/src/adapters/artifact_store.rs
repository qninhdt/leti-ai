use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::error::ArtifactError;
use crate::types::session::SessionId;

/// Pointer to a stored artifact (image upload, large tool output, etc).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub session_id: SessionId,
    pub key: String,
    pub size: u64,
    pub mime: Option<String>,
}

/// Stores per-session artifacts, rooted at
/// `<data_dir>/artifacts/<session_id>/<key>`.
#[async_trait]
pub trait ArtifactStore: Send + Sync + 'static {
    async fn put(
        &self,
        session: SessionId,
        key: &str,
        bytes: Bytes,
    ) -> Result<ArtifactRef, ArtifactError>;

    async fn get(&self, r: &ArtifactRef) -> Result<Bytes, ArtifactError>;

    async fn list(&self, session: SessionId) -> Result<Vec<ArtifactRef>, ArtifactError>;
}
