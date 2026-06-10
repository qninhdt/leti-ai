//! SSE byte-stream → `ChatDelta` decoder task.
//!
//! Extracted from `provider.rs` so the streaming-loop logic lives next
//! to its own helpers (idle timeout, cancellation, parser drain) and
//! `provider.rs` stays focused on HTTP send + response mapping.

use std::time::Duration;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use openlet_core::adapters::model_provider::ChatDelta;
use openlet_core::error::ProviderError;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use super::chunk_decoder::decode_chunk;
use super::sse::SseParser;

/// Idle timeout while waiting for the next byte chunk. Mirrors
/// `STREAM_IDLE_TIMEOUT_MS` in `provider.rs`.
const STREAM_IDLE_TIMEOUT_MS: u64 = 60_000;
/// Backpressure cap for the decoded delta channel. Mirrors
/// `DELTA_CHANNEL_CAPACITY` in `provider.rs`.
const DELTA_CHANNEL_CAPACITY: usize = 64;

/// Spawn a decoder task: reads `reqwest::Response::bytes_stream`, runs
/// the SSE parser + chunk decoder, forwards `ChatDelta` items into an
/// mpsc. The returned receiver is wrapped as a `Stream`.
pub(super) fn spawn_decoder<S>(
    mut bytes_stream: S,
    cancel: CancellationToken,
) -> impl Stream<Item = Result<ChatDelta, ProviderError>> + Send + Unpin + 'static
where
    S: Stream<Item = reqwest::Result<Bytes>> + Send + Unpin + 'static,
{
    let (tx, rx) = mpsc::channel::<Result<ChatDelta, ProviderError>>(DELTA_CHANNEL_CAPACITY);
    let cancel_inner = cancel.clone();

    tokio::spawn(async move {
        let mut parser = SseParser::new();
        let idle = Duration::from_millis(STREAM_IDLE_TIMEOUT_MS);

        loop {
            let next = tokio::time::timeout(idle, bytes_stream.next());
            let chunk = tokio::select! {
                () = cancel_inner.cancelled() => {
                    let _ = tx.send(Err(ProviderError::Cancelled)).await;
                    return;
                }
                res = next => match res {
                    Ok(Some(Ok(bytes))) => bytes,
                    Ok(Some(Err(e))) => {
                        let _ = tx.send(Err(ProviderError::Network(e.to_string()))).await;
                        return;
                    }
                    Ok(None) => break,
                    Err(_) => {
                        let _ = tx
                            .send(Err(ProviderError::Network("idle timeout".into())))
                            .await;
                        return;
                    }
                }
            };

            let frames = match parser.push(&chunk) {
                Ok(f) => f,
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            };

            for frame in frames {
                if frame.is_done() {
                    return;
                }
                if frame.is_heartbeat() || frame.data.is_empty() {
                    continue;
                }
                match decode_chunk(&frame.data) {
                    Ok(deltas) => {
                        for d in deltas {
                            if tx.send(Ok(d)).await.is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        return;
                    }
                }
            }
        }

        // Drain trailing frame if upstream closed without a blank line.
        if let Ok(tail) = parser.finish() {
            for frame in tail {
                if frame.is_done() || frame.is_heartbeat() || frame.data.is_empty() {
                    continue;
                }
                if let Ok(deltas) = decode_chunk(&frame.data) {
                    for d in deltas {
                        if tx.send(Ok(d)).await.is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });

    ReceiverStream::new(rx)
}
