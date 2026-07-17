//! Awaitable resolver for an outstanding permission ask.
//!
//! When `PermissionManager::check` returns `Decision::Pending { ask_id }`,
//! the runtime parks on a `Deferred<Decision>` until the user replies
//! (TUI button, HTTP route, plugin override). The manager owns the
//! sender side; the caller awaits the receiver. Drop on the sender side
//! resolves to `Decision::Deny { feedback: Some("ask cancelled") }` so
//! callers never hang on a lost reply.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::sync::oneshot;

use super::Decision;

/// Receiver half — `await` this to obtain the user's decision.
#[must_use = "Deferred must be awaited or the caller will block forever"]
pub struct Deferred<T> {
    rx: oneshot::Receiver<T>,
    on_drop: T,
}

impl<T> Deferred<T> {
    pub fn new(rx: oneshot::Receiver<T>, on_drop: T) -> Self {
        Self { rx, on_drop }
    }
}

/// Sender half — held by the `PermissionManager` keyed by `AskId`. The
/// manager calls `send` from `reply` / `cancel_ask`. Cloning is not
/// supported; only one resolution is allowed.
pub struct DeferredSender<T> {
    tx: oneshot::Sender<T>,
}

impl<T> DeferredSender<T> {
    #[must_use]
    pub fn new(tx: oneshot::Sender<T>) -> Self {
        Self { tx }
    }

    /// Resolve the deferred. Returns the value back if the receiver was
    /// already dropped.
    pub fn send(self, value: T) -> Result<(), T> {
        self.tx.send(value)
    }
}

/// Helper constructor — pairs a `Deferred` and `DeferredSender` keyed by
/// the same channel. The default value is what the deferred will resolve
/// to if the sender is dropped without explicit `send`.
pub fn deferred_pair(default_on_drop: Decision) -> (Deferred<Decision>, DeferredSender<Decision>) {
    let (tx, rx) = oneshot::channel();
    (Deferred::new(rx, default_on_drop), DeferredSender::new(tx))
}

impl Future for Deferred<Decision> {
    type Output = Decision;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.rx).poll(cx) {
            Poll::Ready(Ok(v)) => Poll::Ready(v),
            Poll::Ready(Err(_)) => Poll::Ready(self.on_drop.clone()),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolves_to_sent_value() {
        let (deferred, sender) = deferred_pair(Decision::Deny { feedback: None });
        sender.send(Decision::Allow).unwrap();
        assert!(matches!(deferred.await, Decision::Allow));
    }

    #[tokio::test]
    async fn drop_resolves_to_default() {
        let default = Decision::Deny {
            feedback: Some("orphaned".into()),
        };
        let (deferred, sender) = deferred_pair(default);
        drop(sender);
        match deferred.await {
            Decision::Deny { feedback } => assert_eq!(feedback.as_deref(), Some("orphaned")),
            other => panic!("expected deny, got {other:?}"),
        }
    }
}
