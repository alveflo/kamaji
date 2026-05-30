//! `GET /events` — Server-Sent Events. Subscribes to the daemon's broadcast and
//! frames each `Event` as a named SSE record: the `event:` line is the dotted
//! `sse_name()`, the `data:` line is the event payload as JSON (the inner
//! `data` of the tagged representation, without the `type` envelope).

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::{Stream, StreamExt};
use kamaji_core::events::Event;
use tokio_stream::wrappers::BroadcastStream;

use crate::state::AppState;

/// Extract the payload (the inner `data`) from a tagged `Event` for the SSE
/// `data:` line. The enum serializes as `{"type":..,"data":..}`; we send only
/// the `data` object, because the `event:` line already carries the name.
fn payload_json(event: &Event) -> String {
    let full = serde_json::to_value(event).unwrap_or(serde_json::Value::Null);
    let data = full.get("data").cloned().unwrap_or(serde_json::Value::Null);
    serde_json::to_string(&data).unwrap_or_else(|_| "null".to_string())
}

fn to_sse(event: &Event) -> SseEvent {
    SseEvent::default()
        .event(event.sse_name())
        .data(payload_json(event))
}

/// `GET /events` → an SSE stream of board deltas. Lossy by design: a client that
/// lags past the channel capacity misses the dropped events but the stream
/// continues with newer ones — the client re-syncs the gap via a re-fetch.
pub async fn events(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = state.tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| async move {
        match result {
            Ok(event) => Some(Ok(to_sse(&event))),
            // Lagged: the client fell behind and `n` events were dropped. Skip
            // this marker (the stream continues with newer events); the client
            // re-syncs the gap via a re-fetch. Log it for observability.
            Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(n)) => {
                tracing::debug!(dropped = n, "SSE client lagged; dropped events");
                None
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
}
