//! Capped streaming reader for stdout/stderr.
//!
//! Reads asynchronously up to `limit` bytes. Anything past the cap is
//! discarded so we never OOM on `yes` / `head -c 10G`. Returns
//! `(bytes_read, truncated)`.

use tokio::io::{AsyncRead, AsyncReadExt};

pub(crate) async fn read_capped<R>(reader: &mut R, limit: usize) -> std::io::Result<(Vec<u8>, bool)>
where
    R: AsyncRead + Unpin,
{
    let mut buf = Vec::with_capacity(limit.min(64 * 1024));
    let mut tmp = [0u8; 8 * 1024];
    let mut truncated = false;
    loop {
        let n = reader.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        let remaining = limit.saturating_sub(buf.len());
        if remaining == 0 {
            // Drain stdout silently — child stays unblocked but we discard.
            truncated = true;
            continue;
        }
        let take = n.min(remaining);
        buf.extend_from_slice(&tmp[..take]);
        if take < n {
            truncated = true;
        }
    }
    Ok((buf, truncated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn caps_at_limit() {
        let payload = vec![b'a'; 1024];
        let cursor = std::io::Cursor::new(payload.clone());
        // Cursor implements AsyncRead via tokio when wrapped, but
        // std::io::Cursor doesn't. Use tokio's duplex.
        let (mut tx, mut rx) = tokio::io::duplex(8 * 1024);
        tx.write_all(&payload).await.unwrap();
        drop(tx);
        let (out, truncated) = read_capped(&mut rx, 256).await.unwrap();
        assert_eq!(out.len(), 256);
        assert!(truncated);
        let _ = cursor;
    }
}
