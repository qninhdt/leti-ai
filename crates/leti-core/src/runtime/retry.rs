//! Bounded retry policy for the runtime↔provider boundary.
//!
//! Retries only transient failures (rate-limit, network/5xx);
//! auth/decode/4xx bubble immediately. Honors a server `Retry-After`
//! over the computed backoff.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::adapters::model_provider::{ChatRequest, ChatStream};
use crate::error::{CoreError, ProviderError};
use crate::types::session::SessionId;

use super::ConversationRuntime;

/// Bounded retry policy for the runtime↔provider boundary. Retries only
/// transient failures (rate-limit, network/5xx); auth/decode/4xx bubble
/// immediately. Honors a server `Retry-After` over the computed backoff.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Max chat-stream open attempts (1 = no retry). Default 4.
    pub max_attempts: u32,
    /// Base backoff; attempt `n` waits `base * 2^(n-1)` plus jitter,
    /// unless the error carried a `Retry-After`. Default 250ms.
    pub base_delay: Duration,
    /// Hard ceiling on cumulative sleep across all retries. Default 30s.
    pub total_deadline: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        let env_u64 = |k: &str, d: u64| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(d)
        };
        Self {
            max_attempts: env_u64("LETI_PROVIDER_RETRY_MAX_ATTEMPTS", 4) as u32,
            base_delay: Duration::from_millis(env_u64("LETI_PROVIDER_RETRY_BASE_MS", 250)),
            total_deadline: Duration::from_millis(env_u64("LETI_PROVIDER_RETRY_TOTAL_MS", 30_000)),
        }
    }
}

/// Apply ±25% jitter to a backoff duration so concurrent retriers don't
/// resynchronize into a thundering herd. Cheap nanosecond-clock entropy —
/// no `rand` dependency for a non-cryptographic spread.
pub(crate) fn jittered(base: Duration) -> Duration {
    let nanos = base.as_nanos() as u64;
    if nanos == 0 {
        return base;
    }
    let spread = nanos / 2; // full jitter band = 50% of base (±25%).
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let offset = seed % spread.max(1);
    // Center the band: base - 25% + offset(0..50%).
    let low = nanos.saturating_sub(spread / 2);
    Duration::from_nanos(low.saturating_add(offset))
}

impl ConversationRuntime {
    /// Open a chat stream with bounded retry on transient provider
    /// failures (rate-limit, network/5xx). Auth/decode/4xx errors bubble
    /// immediately. Honors a server `Retry-After` over computed backoff;
    /// caps both attempt count and cumulative sleep. Cancellation aborts
    /// the wait between attempts.
    #[tracing::instrument(
        skip_all,
        fields(session_id = %session_id, model = %req.model)
    )]
    pub(crate) async fn chat_stream_with_retry(
        &self,
        session_id: SessionId,
        req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, CoreError> {
        let cfg = &self.config.retry;
        let mut slept = Duration::ZERO;
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            match self.provider.chat_stream(req.clone(), cancel.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    let last = attempt >= cfg.max_attempts;
                    if last || !e.is_retryable() {
                        return Err(CoreError::Provider(e));
                    }
                    // Prefer the server's Retry-After; else exponential
                    // backoff (base * 2^(attempt-1)) with ±25% jitter.
                    let backoff = match e.retry_after_ms() {
                        Some(ms) => Duration::from_millis(ms),
                        None => {
                            let exp = cfg.base_delay.saturating_mul(1u32 << (attempt - 1).min(16));
                            jittered(exp)
                        }
                    };
                    // Respect the cumulative deadline: if this sleep would
                    // breach it, stop retrying and surface the error.
                    if slept.saturating_add(backoff) > cfg.total_deadline {
                        return Err(CoreError::Provider(e));
                    }
                    metrics::counter!("leti_provider_retries_total").increment(1);
                    tracing::warn!(
                        session_id = %session_id,
                        attempt,
                        backoff_ms = backoff.as_millis() as u64,
                        class = e.class().as_str(),
                        "provider call failed; retrying after backoff"
                    );
                    tokio::select! {
                        () = cancel.cancelled() => {
                            return Err(CoreError::Provider(ProviderError::Cancelled));
                        }
                        () = tokio::time::sleep(backoff) => {}
                    }
                    slept = slept.saturating_add(backoff);
                }
            }
        }
    }
}
