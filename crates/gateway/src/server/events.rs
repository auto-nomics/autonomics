//! Server-sent event stream handler.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, header};
use axum::response::sse;
use futures::stream::unfold;
use tokio_util::sync::CancellationToken;

use super::state::GatewayState;
use crate::hub::{EventHub, envelope_to_sse, lag_to_sse};
use crate::proto::*;

// ── SSE event stream ─────────────────────────────────────────────────

#[derive(serde::Deserialize, Default)]
pub(crate) struct EventsParams {
    /// Same purpose as the `Last-Event-ID` header, as a query param for
    /// clients that cannot set headers on reconnecting streams.
    #[serde(default)]
    last_event_id: Option<u64>,
}

struct EventStreamState {
    rx: tokio::sync::broadcast::Receiver<Arc<crate::hub::Envelope>>,
    replay: std::collections::VecDeque<Arc<crate::hub::Envelope>>,
    hub: EventHub,
    shutdown: CancellationToken,
    pending_lag: Option<sse::Event>,
}

#[utoipa::path(get, path = "/api/v1/events", tag = "hydration", responses((status = 200, description = "Server-sent event stream", content_type = "text/event-stream")))]
pub(crate) async fn get_events(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Query(params): Query<EventsParams>,
) -> sse::Sse<impl futures::Stream<Item = std::result::Result<sse::Event, std::convert::Infallible>>>
{
    let last_id = headers
        .get(header::HeaderName::from_static("last-event-id"))
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .or(params.last_event_id)
        .unwrap_or(0);

    let hub = state.hub.clone();
    let shutdown = state.shutdown.clone();

    // Subscribe FIRST so nothing published between the replay snapshot
    // and the subscription is lost (duplicates are fine — clients dedup
    // by seq).
    let rx = hub.subscribe();
    let (replay, complete) = hub.replay_after(last_id);
    let pending_lag = (!complete).then(|| {
        lag_to_sse(LagFrame {
            missed: 0,
            resume_seq: hub.last_seq(),
        })
    });
    let st = EventStreamState {
        rx,
        replay: replay.into(),
        hub,
        shutdown,
        pending_lag,
    };

    let stream = unfold(st, |mut st| async move {
        if let Some(lag) = st.pending_lag.take() {
            return Some((Ok(lag), st));
        }
        if let Some(envelope) = st.replay.pop_front() {
            return Some((Ok(envelope_to_sse(&envelope)), st));
        }
        // `unfold` re-invokes this closure for every item, so a single
        // `select!` here is the loop.
        tokio::select! {
            _ = st.shutdown.cancelled() => None,
            recv = st.rx.recv() => match recv {
                Ok(envelope) => Some((Ok(envelope_to_sse(&envelope)), st)),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    Some((Ok(lag_to_sse(LagFrame {
                        missed,
                        resume_seq: st.hub.last_seq(),
                    })), st))
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
            },
        }
    });

    sse::Sse::new(stream).keep_alive(
        sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}
