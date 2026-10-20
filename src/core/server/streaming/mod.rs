//! One committed generation on its way to the caller as server-sent events.
//!
//! [`chunks`] owns the OpenAI chat-chunk encoder and [`pump`] drives it; the
//! two other formats bring their own encoders from [`crate::core::wire`] and
//! share the response wrapper and the end-of-stream accounting below.

pub(in crate::core::server) mod chunks;
mod pump;

use std::sync::atomic::Ordering;
use std::time::Instant;

use axum::response::{IntoResponse, Response};

use crate::core::server::telemetry::{TOTAL_FAILURES, TOTAL_INPUT_TOKENS, TOTAL_OUTPUT_TOKENS};
use crate::providers::stream::StopReason;

/// Milliseconds in a second, for a latency read off a monotonic clock.
const MS_PER_SECOND: f64 = 1_000.0; // https://www.bipm.org/en/measurement-units/si-prefixes milli = 10^-3

/// One failed stream, as the failure total counts it.
const ONE_FAILURE: u64 = std::num::NonZeroU64::MIN.get();

/// This host's clock in whole epoch seconds, the `created` an answer states,
/// or the refusal a clock set before the epoch earns: no creation time can be
/// stated from it.
pub(in crate::core::server) fn epoch_seconds() -> Result<u64, String> {
    let now = chrono::Utc::now().timestamp();
    u64::try_from(now).map_err(|error| {
        format!("this host's clock reads {now}, before the Unix epoch, so no creation time can be stated: {error}")
    })
}

/// Wrap one encoder in the SSE response every streaming format shares.
///
/// A frame is sent when the provider produces one; the gateway adds no frames
/// of its own on a clock nobody declared.
pub(in crate::core::server) fn sse_response<S>(stream: S) -> Response
where
    S: futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>
        + Send
        + 'static,
{
    axum::response::sse::Sse::new(stream).into_response()
}

/// The closure a wire encoder reports its stream's end through.
///
/// Statistics belong to this layer; the encoders know the numbers but not
/// where they are kept, so they are handed this instead of a static they would
/// have to reach into. Tokens are added only when the provider stated them,
/// and a model's throughput is recorded only from a stated output count.
pub(in crate::core::server) fn stream_accounting(
    requested_model: String,
    started: Instant,
) -> crate::core::wire::StreamAccounting {
    Box::new(move |usage, failed| {
        if let Some((input_tokens, output_tokens)) = usage {
            TOTAL_INPUT_TOKENS.fetch_add(u64::from(input_tokens), Ordering::Relaxed);
            TOTAL_OUTPUT_TOKENS.fetch_add(u64::from(output_tokens), Ordering::Relaxed);
        }
        if failed {
            TOTAL_FAILURES.fetch_add(ONE_FAILURE, Ordering::Relaxed);
        } else if let Some((_, output_tokens)) = usage {
            crate::core::perf::record(
                &requested_model,
                started.elapsed().as_secs_f64() * MS_PER_SECOND,
                output_tokens,
            );
        }
    })
}

/// A provider stop reason in the OpenAI finish vocabulary; a reason it has no
/// word for is passed on as the provider stated it.
pub(super) fn openai_finish_reason(reason: &StopReason) -> String {
    match reason {
        StopReason::EndTurn | StopReason::StopSequence => "stop".to_string(),
        StopReason::MaxTokens => "length".to_string(),
        StopReason::ToolUse => "tool_calls".to_string(),
        StopReason::ContentFilter => "content_filter".to_string(),
        StopReason::Incomplete(Some(stated)) | StopReason::Other(stated) => stated.clone(),
        StopReason::Incomplete(None) => "incomplete".to_string(),
    }
}
