//! `GET /v1/event` — single multiplexed SSE channel.
//!
//! Frame format: `id:<events.id>\nevent:<kind>\ndata:<json>\n\n`. Transient
//! events (`part.delta`, `heartbeat`) skip the `id:` line because no
//! durable autoincrement exists for them. `Last-Event-ID` header
//! (header-only, no query alias) drives replay; we read
//! durable rows with `id > last_event_id` and prepend before falling
//! through to the live broadcast subscription.
//!
//! Heartbeat is wired through `axum::response::sse::KeepAlive`; we keep
//! the cadence at 15s.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Sse;
use axum::response::sse::{Event, KeepAlive};
use futures::stream::{self, Stream, StreamExt};
use openlet_core::adapters::event_sink::DeliveredEvent;
use openlet_core::types::event::EventFilter;
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
            content_type = "text/event-stream"),
        (status = 400, description = "Malformed Last-Event-ID header"),
    )
)]
pub async fn stream(
    State(state): State<AppState>,
    Query(query): Query<EventQuery>,
    headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let session_filter = query.session.map(SessionId::from);
    // Distinguish "header absent" (None — fresh subscribe) from "header
    // present but unparseable" (400). Silently treating malformed as
    // absent meant the client would believe it had resumed but actually
    // missed every event since its last id.
    let last_event_id = match headers.get("Last-Event-ID") {
        None => None,
        Some(v) => {
            let s = v.to_str().map_err(|_| {
                AppError::bad_request(
                    "invalid_last_event_id",
                    "Last-Event-ID header is not valid UTF-8",
                )
            })?;
            let id = s.parse::<i64>().map_err(|_| {
                AppError::bad_request(
                    "invalid_last_event_id",
                    "Last-Event-ID header must be a non-negative integer",
                )
            })?;
            Some(id)
        }
    };

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

    // Live frames are wrapped so we can emit a synthetic `lagged`
    // signal when the broadcast channel reports `Lagged(n)`. Without
    // it, slow consumers silently miss events: the heartbeat keeps the
    // EventSource open, but no replay ever fires. The lagged frame
    // gives the client a deterministic cue to reconnect with
    // `Last-Event-ID` and replay the durable tail.
    enum LiveItem {
        Event(DeliveredEvent),
        Lagged(u64),
    }

    let live = BroadcastStream::new(receiver).filter_map(move |res| async move {
        match res {
            Ok(d) => {
                if matches!(d.event_id, Some(id) if id <= replay_high_water) {
                    None
                } else {
                    Some(LiveItem::Event(d))
                }
            }
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                Some(LiveItem::Lagged(n))
            }
        }
    });

    let replay_stream = stream::iter(replay).map(LiveItem::Event);
    let combined = replay_stream.chain(live).filter_map(move |item| {
        let frame = match item {
            LiveItem::Lagged(n) => {
                // Emit regardless of session filter — the client needs
                // to know its cursor advanced past unseen events even
                // for a session-scoped subscription.
                Some(Ok(Event::default()
                    .event("lagged")
                    .data(format!("{{\"missed\":{n}}}"))))
            }
            LiveItem::Event(d) => {
                let allow = match (session_filter, d.event.session_id()) {
                    (Some(want), Some(got)) => want == got,
                    (Some(_), None) => false,
                    (None, _) => true,
                };
                if allow { Some(encode_frame(d)) } else { None }
            }
        };
        async move { frame }
    });

    Ok(Sse::new(combined).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS))
            .text("heartbeat"),
    ))
}

fn encode_frame(d: DeliveredEvent) -> Result<Event, Infallible> {
    let kind = d.event.kind();
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
