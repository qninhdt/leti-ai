use std::pin::Pin;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};

use crate::error::ArtifactError;
use crate::types::session::SessionId;

/// A streamed artifact body. Cloud stores (S3/MinIO) stream large blobs
/// without buffering the whole object; the local default bridges to the
/// existing buffered `get`/`put`.
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, ArtifactError>> + Send>>;

/// Which operation a presigned URL authorizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresignOp {
    /// Direct client download.
    Get,
    /// Direct client upload.
    Put,
}

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

    /// Streamed read. Cloud stores override to stream directly from
    /// object storage; the default buffers via [`Self::get`] and emits
    /// the whole body as one chunk, so test doubles need no change.
    async fn get_stream(&self, r: &ArtifactRef) -> Result<ByteStream, ArtifactError> {
        let bytes = self.get(r).await?;
        Ok(futures::stream::once(async move { Ok(bytes) }).boxed())
    }

    /// Streamed write. The default collects the stream into `Bytes` and
    /// delegates to [`Self::put`]; cloud stores override to stream
    /// straight to object storage. Any per-chunk error aborts the put.
    async fn put_stream(
        &self,
        session: SessionId,
        key: &str,
        mut stream: ByteStream,
    ) -> Result<ArtifactRef, ArtifactError> {
        let mut buf = Vec::new();
        while let Some(chunk) = stream.next().await {
            buf.extend_from_slice(&chunk?);
        }
        self.put(session, key, Bytes::from(buf)).await
    }

    /// Presigned URL for a direct client transfer, when the backend
    /// supports it (S3/MinIO). Local filesystem returns `None` — callers
    /// fall back to streaming through the server.
    fn presign(&self, _r: &ArtifactRef, _op: PresignOp) -> Option<String> {
        None
    }
}
