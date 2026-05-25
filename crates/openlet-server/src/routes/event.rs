//! `GET /v1/event` — single multiplexed SSE channel.
//!
//! Frame format: `id:<events.id>\nevent:<kind>\ndata:<json>\n\n`. Transient
//! events (`part.delta`, `heartbeat`) skip the `id:` line because no
//! durable autoincrement exists for them. `Last-Event-ID` header (per
//! amendment §C — header-only, no query alias) drives replay; we read
//! durable rows with `id > last_event_id` and prepend before falling
//! through to the live broadcast subscription.
//!
//! Heartbeat is wired through `axum::response::sse::KeepAlive`; we keep
//! the cadence at 15s (cross-check phase-05).

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Sse;
use axum::response::sse::{Event, KeepAlive};
use futures::stream::{self, Stream, StreamExt};
use openlet_core::adapters::event_sink::DeliveredEvent;
use openlet_core::types::event::{AgentEvent, EventFilter};
use openlet_core::types::session::SessionId;
use openlet_protocol::EventDto;
use serde::Deserialize;
use tokio_stream::wrappers::BroadcastStream;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::error::AppError;

const HEARTBEAT_INTERVAL_SECS: u64 = 15;

#[derive(Debug, Deserialize)]
pub struct EventQuery {
    /// Optional session-id filter; absent = global stream.
    pub session: Option<Uuid>,
}

#[utoipa::path(
    get,
    path = "/v1/event",
    tag = "event",
    params(
        ("session" = Option<Uuid>, Query, description = "Filter to one session"),
    ),
    responses(
        (status = 200, description = "SSE stream of AgentEvent frames", body = String,
            content_type = "text/event-stream")
    )
)]
pub async fn stream(
    State(state): State<AppState>,
    Query(query): Query<EventQuery>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let session_filter = query.session.map(SessionId::from);
    let last_event_id = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<i64>().ok());

    // Subscribe BEFORE the replay query so any event durably written
    // during the replay is buffered on the broadcast receiver and not
    // lost. Replay + live then overlap on the seam; we dedupe by
    // event_id below.
    let receiver = state.events.subscribe(EventFilter {
        session_id: session_filter,
        include_transient: true,
    });

    let replay: Vec<DeliveredEvent> = match (session_filter, last_event_id) {
        (Some(sid), Some(after)) => state.events.replay_since(sid, after).await?,
        // Global SSE with Last-Event-ID — query the unfiltered durable
        // log so a reconnecting global subscriber doesn't drop events.
        (None, Some(after)) => state.events.replay_since_global(after).await?,
        _ => Vec::new(),
    };

    // High-water mark across replay rows. Live frames at or below this
    // were captured by the replay query and must be dropped to prevent
    // duplicate emission across the subscribe/replay seam.
    let replay_high_water: i64 = replay
        .iter()
        .filter_map(|d| d.event_id)
        .max()
        .unwrap_or(i64::MIN);

    // Slow consumer drops are swallowed — clients reconnect with
    // Last-Event-ID rather than tearing the stream down.
    let live = BroadcastStream::new(receiver)
        .filter_map(|res| async move { res.ok() })
        .filter(move |d| {
            let drop_dup = matches!(d.event_id, Some(id) if id <= replay_high_water);
            async move { !drop_dup }
        });

    let combined = stream::iter(replay).chain(live).filter_map(move |d| {
        let allow = match (session_filter, event_session_id(&d.event)) {
            (Some(want), Some(got)) => want == got,
            (Some(_), None) => false,
            (None, _) => true,
        };
        async move {
            if !allow {
                return None;
            }
            Some(encode_frame(d))
        }
    });

    Ok(Sse::new(combined).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS))
            .text("heartbeat"),
    ))
}

fn encode_frame(d: DeliveredEvent) -> Result<Event, Infallible> {
    let kind = event_kind(&d.event);
    let dto = EventDto::from(d.event);
    let mut frame = Event::default()
        .event(kind)
        .json_data(&dto)
        .unwrap_or_else(|_| Event::default().event("error").data("event encode failure"));
    if let Some(id) = d.event_id {
        frame = frame.id(id.to_string());
    }
    Ok(frame)
}

fn event_kind(ev: &AgentEvent) -> &'static str {
    match ev {
        AgentEvent::SessionStatus { .. } => "session.status",
        AgentEvent::MessageCreated { .. } => "message.created",
        AgentEvent::PartCreated { .. } => "part.created",
        AgentEvent::PartDelta { .. } => "part.delta",
        AgentEvent::PartUpdated { .. } => "part.updated",
        AgentEvent::StepFinished { .. } => "step.finished",
        AgentEvent::PermissionAsked { .. } => "permission.asked",
        AgentEvent::PermissionResolved { .. } => "permission.resolved",
        AgentEvent::Error { .. } => "error",
        AgentEvent::PluginError { .. } => "plugin.error",
        AgentEvent::QuestionRequested { .. } => "question.requested",
        AgentEvent::Heartbeat => "heartbeat",
    }
}

fn event_session_id(ev: &AgentEvent) -> Option<SessionId> {
    match ev {
        AgentEvent::SessionStatus { session_id, .. }
        | AgentEvent::MessageCreated { session_id, .. }
        | AgentEvent::PartCreated { session_id, .. }
        | AgentEvent::PartDelta { session_id, .. }
        | AgentEvent::PartUpdated { session_id, .. }
        | AgentEvent::StepFinished { session_id, .. }
        | AgentEvent::PermissionAsked { session_id, .. }
        | AgentEvent::PermissionResolved { session_id, .. }
        | AgentEvent::QuestionRequested { session_id, .. } => Some(*session_id),
        AgentEvent::Error { session_id, .. } | AgentEvent::PluginError { session_id, .. } => {
            *session_id
        }
        AgentEvent::Heartbeat => None,
    }
}
