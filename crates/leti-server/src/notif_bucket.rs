//! Per-session token bucket rate limiter for plugin notifications.
//!
//! Extracted from `core_api_impl.rs` so the rate-limit logic lives next
//! to its own tests and so `core_api_impl.rs` stays focused on the
//! `CoreApi` trait surface.

use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};

use chrono::Utc;

pub(crate) const NOTIF_BUCKET_CAPACITY: u32 = 10;
/// 100ms per token → 10 tokens/sec. Refill happens lazily on `try_take`.
pub(crate) const NOTIF_REFILL_INTERVAL_MS: i64 = 100;

/// Token bucket: 10 capacity, refill 10/sec. Cumulative across plugins
/// (per-session, not per-plugin) so a single misbehaving plugin can't
/// be hidden by another well-behaved one's quota.
#[derive(Debug)]
pub(crate) struct NotifBucket {
    tokens: AtomicU32,
    last_refill_ms: AtomicI64,
}

impl NotifBucket {
    pub(crate) fn new() -> Self {
        Self {
            tokens: AtomicU32::new(NOTIF_BUCKET_CAPACITY),
            last_refill_ms: AtomicI64::new(Utc::now().timestamp_millis()),
        }
    }

    /// Try to take one token. Refills lazily on each call. Returns
    /// `true` if a token was consumed (allow), `false` if drained
    /// (drop notification).
    pub(crate) fn try_take(&self) -> bool {
        let now_ms = Utc::now().timestamp_millis();
        let last = self.last_refill_ms.load(Ordering::Acquire);
        let elapsed_ms = now_ms.saturating_sub(last);
        if elapsed_ms >= NOTIF_REFILL_INTERVAL_MS {
            // Refill at 10 tokens/sec → cap at capacity.
            let refill = (elapsed_ms / NOTIF_REFILL_INTERVAL_MS).min(i64::from(u32::MAX)) as u32;
            self.last_refill_ms.store(now_ms, Ordering::Release);
            let prev = self.tokens.load(Ordering::Acquire);
            let next = prev.saturating_add(refill).min(NOTIF_BUCKET_CAPACITY);
            self.tokens.store(next, Ordering::Release);
        }
        // CAS decrement.
        loop {
            let cur = self.tokens.load(Ordering::Acquire);
            if cur == 0 {
                return false;
            }
            if self
                .tokens
                .compare_exchange(cur, cur - 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notif_bucket_drops_after_capacity() {
        let bucket = NotifBucket::new();
        // 10 capacity → first 10 succeed.
        for _ in 0..NOTIF_BUCKET_CAPACITY {
            assert!(bucket.try_take(), "should succeed within capacity");
        }
        // 11th in the same instant → drops.
        assert!(
            !bucket.try_take(),
            "11th emit must drop (rate limit triggered)"
        );
    }

    #[test]
    fn notif_bucket_refills_after_interval() {
        let bucket = NotifBucket::new();
        for _ in 0..NOTIF_BUCKET_CAPACITY {
            assert!(bucket.try_take());
        }
        // Force a refill window by rewinding `last_refill_ms` 1 sec.
        bucket
            .last_refill_ms
            .store(Utc::now().timestamp_millis() - 1_000, Ordering::Release);
        // After refill, capacity restores. We don't assert the exact
        // count because the refill formula is `elapsed / interval`;
        // 1 sec / 100ms = 10, capped at capacity.
        assert!(bucket.try_take(), "post-refill emit should succeed");
    }
}
