//! The workspace-wide activity feed: everything JARVIS is doing, in one stream.
//!
//! `GET /api/v1/runs/{id}/events` follows one run, which is the right shape for a chat view and the wrong one for
//! a heads-up display, a voice client or a notifier: those want to see *all* work — which run just started,
//! which is waiting on a person, which just finished — without knowing an identifier first. This is that stream.
//!
//! It is built from the same durable rows the per-run stream reads, so it cannot disagree with them:
//!
//! - **Every frame is a run event** (`run.approval_requested`, `run.completed`, …), spelled by the same renderer,
//!   so a client that understands a run stream understands this one.
//! - **Resume is by cursor.** The SSE `id:` is the position in the workspace's storage order; `Last-Event-ID` or
//!   `?after=` resumes after it, and a client that missed frames re-reads them from the store rather than losing
//!   them. With neither, the stream starts **from now** (a HUD reads `GET /api/v1/runs` for current state, then
//!   tails); `?after=0` replays everything retained.
//! - **Streamed output text is opt-in** (`?deltas=true`): one answer is hundreds of events, and a display wants the
//!   transitions, not the tokens.
//! - **A slow consumer is disconnected, not dropped from**: the same bounded hand-off and overrun frame as a run
//!   stream, so a reconnect with the last cursor loses nothing.
//!
//! The feed polls the store on a short interval rather than subscribing, because a workspace-wide wake-up channel
//! does not exist and the read is one indexed range query; the interval bounds latency, and idle cost is a single
//! `MAX` lookup per tick.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::{RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::http::runs::{
    OverrunSignal, ReceiverStream, accepts_event_stream, context_for, internal_failure,
    invalid_request, not_ready, render_event_with_id, send_bounded, service_error_response,
};
use crate::http::{ApiState, AuthenticatedClient, RequestIdOf, error_response_for};

/// How often an idle feed checks the store.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The depth of the channel between the feed task and the response body.
const FEED_CHANNEL_DEPTH: usize = 32;

/// The options a feed request carries.
struct FeedRequest {
    after: Option<u64>,
    deltas: bool,
}

/// Reads the resume position and options from the request, or says what was wrong.
fn parse_request(headers: &HeaderMap, query: Option<&str>) -> Result<FeedRequest, &'static str> {
    let mut after: Option<u64> = None;
    let mut deltas = false;
    for pair in query
        .unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
    {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "after" => {
                after = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| "`after` must be a cursor the feed returned.")?,
                );
            }
            "deltas" => deltas = value == "true" || value == "1",
            _ => return Err("The query string carries a parameter this endpoint does not accept."),
        }
    }
    // The header wins over the query, as it does on a run stream: it is what a reconnecting client sends.
    if let Some(value) = headers.get("last-event-id") {
        after = Some(
            value
                .to_str()
                .ok()
                .and_then(|text| text.parse::<u64>().ok())
                .ok_or("`Last-Event-ID` must be a cursor the feed returned.")?,
        );
    }
    Ok(FeedRequest { after, deltas })
}

/// Handles `GET /api/v1/activity`.
pub async fn activity_feed(
    State(state): State<Arc<ApiState>>,
    client: AuthenticatedClient,
    RequestIdOf(request_id): RequestIdOf,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let request_id = request_id.as_deref();
    let Some(service) = state.runs.as_ref() else {
        return not_ready(request_id);
    };
    if !accepts_event_stream(&headers) {
        return error_response_for(
            request_id,
            StatusCode::BAD_REQUEST,
            "request.invalid",
            "This endpoint serves `text/event-stream`.",
            false,
        );
    }
    let request = match parse_request(&headers, query.as_deref()) {
        Ok(request) => request,
        Err(message) => return invalid_request(request_id, message),
    };
    let context = context_for(&client, request_id);

    // The first read happens before the body begins, so a storage fault is still a status code, and a request
    // with no position learns "now" here.
    let first = match service
        .activity(&context, request.after, request.deltas)
        .await
    {
        Ok(page) => page,
        Err(error) => return service_error_response(request_id, &error),
    };
    let mut initial = Vec::with_capacity(first.events.len());
    for feed in &first.events {
        match render_event_with_id(&feed.event, &feed.cursor.to_string()) {
            Ok(frame) => initial.push(frame),
            Err(()) => return internal_failure(request_id),
        }
    }

    let (sender, receiver) = tokio::sync::mpsc::channel::<Bytes>(FEED_CHANNEL_DEPTH);
    let (signal, signal_receiver) = OverrunSignal::new();
    let service = Arc::clone(service);
    let keepalive = state.keepalive_interval;
    let overrun = state.stream_overrun_timeout;
    let deltas = request.deltas;
    let mut cursor = first.next_cursor;

    tokio::spawn(async move {
        for frame in initial {
            if !send_bounded(&sender, frame, overrun).await {
                signal.record_overrun();
                return;
            }
        }
        let mut last_sent = Instant::now();
        loop {
            // A failed read ends the stream; the client resumes from its last cursor.
            let Ok(page) = service.activity(&context, Some(cursor), deltas).await else {
                return;
            };
            let busy = !page.events.is_empty();
            for feed in &page.events {
                let Ok(frame) = render_event_with_id(&feed.event, &feed.cursor.to_string()) else {
                    return;
                };
                if !send_bounded(&sender, frame, overrun).await {
                    signal.record_overrun();
                    return;
                }
                last_sent = Instant::now();
            }
            cursor = page.next_cursor;
            if busy {
                // A full page means there may be more waiting; read again without sleeping.
                continue;
            }
            if last_sent.elapsed() >= keepalive {
                if !send_bounded(&sender, jarvis_protocol::run::keepalive_frame(), overrun).await {
                    signal.record_overrun();
                    return;
                }
                last_sent = Instant::now();
            }
            tokio::time::sleep(POLL_INTERVAL.min(keepalive)).await;
        }
    });

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/event-stream"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Body::from_stream(ReceiverStream::new(receiver, signal_receiver)),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderMap;

    use super::parse_request;

    #[test]
    fn no_position_means_from_now_and_the_options_are_read() {
        let none = parse_request(&HeaderMap::new(), None).expect("parses");
        assert_eq!(none.after, None);
        assert!(!none.deltas);
        let full = parse_request(&HeaderMap::new(), Some("after=7&deltas=true")).expect("parses");
        assert_eq!(full.after, Some(7));
        assert!(full.deltas);
    }

    #[test]
    fn the_header_wins_over_the_query_as_it_does_on_a_run_stream() {
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", "42".parse().expect("header"));
        let request = parse_request(&headers, Some("after=7")).expect("parses");
        assert_eq!(request.after, Some(42));
    }

    #[test]
    fn a_bad_cursor_or_an_unknown_parameter_is_refused_not_ignored() {
        assert!(parse_request(&HeaderMap::new(), Some("after=abc")).is_err());
        assert!(parse_request(&HeaderMap::new(), Some("after=-1")).is_err());
        assert!(parse_request(&HeaderMap::new(), Some("workspace=other")).is_err());
        let mut headers = HeaderMap::new();
        headers.insert("last-event-id", "not-a-cursor".parse().expect("header"));
        assert!(parse_request(&headers, None).is_err());
    }
}
