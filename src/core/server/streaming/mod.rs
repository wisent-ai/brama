//! One committed generation on its way to the caller as server-sent events.
//!
//! [`chunks`] owns the OpenAI chat-chunk encoder and [`pump`] drives it; the
//! two other formats bring their own encoders from [`crate::core::wire`] and
//! share the response wrapper and the end-of-stream accounting below.

pub(in crate::core::server) mod chunks;
mod pump;

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axum::response::{IntoResponse, Response};

use crate::core::server::telemetry::{TOTAL_FAILURES, TOTAL_INPUT_TOKENS, TOTAL_OUTPUT_TOKENS};

pub(in crate::core::server) fn epoch_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Wrap one encoder in the SSE response every streaming format shares.
///
/// The keep-alive comment is what stops an idle proxy from closing a stream
/// that is legitimately silent while the model thinks; it is a comment frame,
/// so no client parses it as content.
pub(in crate::core::server) fn sse_response<S>(stream: S) -> Response
where
    S: futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>
        + Send
        + 'static,
{
    axum::response::sse::Sse::new(stream)
        .keep_alive(
            axum::response::sse::KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

/// The closure a wire encoder reports its stream's end through.
///
/// Statistics belong to this layer; the encoders in [`crate::core::wire`] know
/// the numbers but not where they are kept, so they are handed this instead of
/// a static they would have to reach into.
pub(in crate::core::server) fn stream_accounting(
    requested_model: String,
    started: Instant,
) -> crate::core::wire::StreamAccounting {
    Box::new(move |input_tokens, output_tokens, failed| {
        TOTAL_INPUT_TOKENS.fetch_add(u64::from(input_tokens), Ordering::Relaxed);
        TOTAL_OUTPUT_TOKENS.fetch_add(u64::from(output_tokens), Ordering::Relaxed);
        if failed {
            TOTAL_FAILURES.fetch_add(u64::from(true), Ordering::Relaxed);
        } else {
            crate::core::perf::record(
                &requested_model,
                started.elapsed().as_secs_f64() * 1_000.0,
                output_tokens,
            );
        }
    })
}

/// Map a provider stop reason onto the OpenAI finish vocabulary.
pub(super) fn openai_finish_reason(reason: Option<&str>, saw_tool_calls: bool) -> String {
    match reason {
        Some("end_turn") | Some("stop_sequence") | Some("stop") => "stop",
        Some("max_tokens") => "length",
        Some("tool_use") | Some("tool_calls") => "tool_calls",
        Some("length") | Some("content_filter") => reason.unwrap_or("stop"),
        Some(other) if other.starts_with("incomplete") => {
            if other.contains("max_output_tokens") {
                "length"
            } else {
                "stop"
            }
        }
        _ => {
            if saw_tool_calls {
                "tool_calls"
            } else {
                "stop"
            }
        }
    }
    .to_string()
}
