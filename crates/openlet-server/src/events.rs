//! Internal event publishing helpers.
//!
//! Centralises the `AgentEvent::SessionStatus { … } / Persistence::Durable`
//! pattern that previously lived in 6 sites (main, message, session,
//! cancel, core_api_impl). Keeping the shape in one place removes drift
//! and makes the `Cancelling`-emit invariant easier to audit.

use std::sync::Arc;

use chrono::Utc;
use openlet_core::adapters::event_sink::{EventSink, Persistence};
use openlet_core::types::event::AgentEvent;
use openlet_core::types::session::{SessionId, SessionStatus};

/// Publish a durable `SessionStatus` event for `sid`. Errors are logged
/// at warn and swallowed — every existing caller already used `let _ =`
/// because status emission is best-effort against a downed bus.
pub async fn publish_status(events: &Arc<dyn EventSink>, sid: SessionId, status: SessionStatus) {
    if let Err(err) = events
        .publish(
            AgentEvent::SessionStatus {
                session_id: sid,
                status,
                at: Utc::now(),
            },
            Persistence::Durable,
        )
        .await
    {
        tracing::warn!(session = %sid, ?status, error = %err, "publish_status failed");
    }
}
