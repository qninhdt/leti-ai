//! Graceful shutdown helpers — turn drain + plugin teardown + signal.
//!
//! Extracted from `main.rs` for testability and to keep the binary
//! entry point focused on wiring.

use std::time::Duration;

use dashmap::DashMap;
use futures::FutureExt;
use leti_core::types::session::SessionId;
use leti_plugin_registry::PluginHandles;
use tracing::info;

use crate::app_state::TurnHandle;

/// Outcome of the turn-drain phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainResult {
    /// All in-flight turns exited cleanly within the timeout.
    Drained,
    /// The timeout elapsed before all turns exited.
    TimedOut,
    /// No turns were in-flight; nothing to drain.
    NoneInFlight,
}

/// Cancel every in-flight turn and await their exit signals under
/// `timeout`. Returns whether all turns exited cleanly.
///
/// The cancel-then-await pattern:
/// 1. Subscribe to each turn's `exited` Notify BEFORE tripping cancel so
///    the driver's `notify_waiters()` can't slip through the gap.
/// 2. Trip `cancel` on each turn.
/// 3. Await all `exited` signals under a single timeout budget.
pub async fn drain_in_flight_turns(
    active_turns: &DashMap<SessionId, TurnHandle>,
    timeout: Duration,
) -> DrainResult {
    let in_flight: Vec<_> = active_turns.iter().map(|e| e.value().clone()).collect();
    if in_flight.is_empty() {
        return DrainResult::NoneInFlight;
    }

    info!(count = in_flight.len(), "draining in-flight turns");
    let drain = async {
        let waits = in_flight.into_iter().map(|h| async move {
            let n = h.exited.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            h.cancel.cancel();
            n.await;
        });
        futures::future::join_all(waits).await;
    };

    if tokio::time::timeout(timeout, drain).await.is_err() {
        tracing::warn!(
            timeout_secs = timeout.as_secs(),
            "turn drain timed out; some in-flight turns may not have finished cleanly"
        );
        return DrainResult::TimedOut;
    }

    DrainResult::Drained
}

/// Shut down all plugins in parallel under `timeout`. Each plugin's
/// shutdown is panic-isolated so a buggy plugin cannot strand the others.
pub async fn shutdown_plugins(registry: &PluginHandles, timeout: Duration) {
    let shutdowns = registry.iter().map(|plugin| {
        let id = plugin.manifest().id.clone();
        async move {
            let result = tokio::time::timeout(
                timeout,
                std::panic::AssertUnwindSafe(plugin.shutdown()).catch_unwind(),
            )
            .await;
            match result {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(e))) => {
                    tracing::warn!(plugin = %id, error = %e, "plugin shutdown returned error");
                }
                Ok(Err(_)) => {
                    tracing::warn!(plugin = %id, "plugin shutdown panicked");
                }
                Err(_) => {
                    tracing::warn!(plugin = %id, timeout_secs = timeout.as_secs(), "plugin shutdown timed out");
                }
            }
        }
    });
    futures::future::join_all(shutdowns).await;
}

/// Wait for Ctrl+C or SIGTERM, then return. Used as
/// `axum::serve(...).with_graceful_shutdown(shutdown_signal())`.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler");
    };

    #[cfg(unix)]
    let term = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => info!("received Ctrl+C, shutting down"),
        () = term => info!("received SIGTERM, shutting down"),
    }
}
