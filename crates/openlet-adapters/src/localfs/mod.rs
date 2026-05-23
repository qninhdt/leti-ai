//! Local-filesystem `ArtifactStore` impl.
//!
//! Phase 1 stub. Phase 2 implements with `<data_dir>/artifacts/<session>/<key>`.

use async_trait::async_trait;
use bytes::Bytes;
use openlet_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use openlet_core::error::ArtifactError;
use openlet_core::types::session::SessionId;

#[derive(Debug, Default)]
pub struct LocalFsArtifactStore;

impl LocalFsArtifactStore {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ArtifactStore for LocalFsArtifactStore {
    async fn put(
        &self,
        _session: SessionId,
        _key: &str,
        _bytes: Bytes,
    ) -> Result<ArtifactRef, ArtifactError> {
        Err(ArtifactError::Unimplemented)
    }

    async fn get(&self, _r: &ArtifactRef) -> Result<Bytes, ArtifactError> {
        Err(ArtifactError::Unimplemented)
    }

    async fn list(
        &self,
        _session: SessionId,
    ) -> Result<Vec<ArtifactRef>, ArtifactError> {
        Err(ArtifactError::Unimplemented)
    }
}
