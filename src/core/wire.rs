//! Native wire formats at the ingress: Anthropic Messages and OpenAI
//! Responses, translated once here and nowhere else.
//!
//! [`super::server`] speaks OpenAI chat completions natively; this module is
//! what lets a client that speaks the other two first-party formats reach the
//! same routing decision without a protocol shim in between. The translation
//! is deliberately total and lossy in one direction each way: inbound, only
//! what a [`ModelRequest`] can hold is kept (stop sequences, cache-control
//! hints and provider-side tool types have no home there and are dropped, not
//! invented); outbound, only what the provider actually said is emitted (a
//! stream that produced no usage chunk produces no usage numbers).
//!
//! Streaming encoders follow the same rule the OpenAI one does: a provider
//! failure after the first byte ends the stream without its terminal event --
//! `message_stop`, `response.completed` -- because the generation the caller
//! holds is incomplete and no later event may pretend otherwise.
#![allow(unused_imports)]

mod inbound_call_part;
mod responses_response_part;
mod sse_part;
mod responses_event_stream_part;
mod part5;

pub use inbound_call_part::*;
pub use responses_response_part::*;
pub use sse_part::*;
pub use responses_event_stream_part::*;
pub use part5::*;

use serde_json::{json, Value};
use crate::providers::stream::{StreamDelta, StreamItem};
use crate::types::{Message, ModelRequest, ModelResponse, Tool, ToolCall, ToolFunction};
use axum::response::sse::Event;
