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
//!
//! What stays in this file is what does not depend on which format asked: the
//! call shape a request is reduced to, the bounds every inbound request is
//! held to, the SSE framing both encoders write through, and the two buffered
//! answers a caller who did not ask for a stream receives. `anthropic_call`
//! and `responses_call` each hold one format's reading of a request, and
//! `anthropic_events` and `responses_events` each hold one format's live event
//! stream -- so a vendor that reshapes its request or renames its events is
//! one file, and nothing here has to be reread to see that the other format is
//! unaffected.

mod anthropic_call;
mod anthropic_events;
mod responses_call;
mod responses_events;

use serde_json::{json, Value};

use crate::types::{ModelRequest, ModelResponse, ToolCall};
use axum::response::sse::Event;

pub use anthropic_call::anthropic_request;
pub use anthropic_events::AnthropicEventStream;
pub use responses_call::responses_request;
pub use responses_events::ResponsesEventStream;

/// What an encoder reports once, when its stream ends: the provider's own
/// token meters and whether the generation completed.
///
/// The encoders live here, but process statistics belong to the HTTP layer
/// that owns them, so the layer hands each stream a closure instead of this
/// module reaching for a static it does not own.
pub type StreamAccounting = Box<dyn FnMut(u32, u32, bool) + Send>;

/// One parsed inbound call, format already erased.
pub struct InboundCall {
    pub model: String,
    pub request: ModelRequest,
    pub stream: bool,
}

fn default_temperature() -> f64 {
    0.7
}

fn default_max_tokens() -> u32 {
    1024
}

/// Shared inbound bounds, identical to the chat-completions contract so the
/// answer does not depend on which format the caller speaks.
fn validate(model: &str, max_tokens: u32, temperature: f64) -> Result<(), String> {
    if model.trim().is_empty() {
        return Err("missing field `model`".to_string());
    }
    if max_tokens == u32::default() || max_tokens > 32_768 {
        return Err("max_tokens must be between one and 32768".to_string());
    }
    if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
        return Err("temperature must be finite and between zero and 2".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Buffered egress.
// ---------------------------------------------------------------------------

fn tool_calls_from_response(response: &ModelResponse) -> Vec<ToolCall> {
    response.tool_calls.clone().unwrap_or_default()
}

/// Shape one buffered internal response as an Anthropic message.
///
/// `stop_reason` is derived, not reported: the internal response does not
/// carry the provider's reason, so a tool call reads as `tool_use` and
/// anything else as `end_turn` -- the two readings a caller can act on.
pub fn anthropic_response(id: &str, model: &str, response: &ModelResponse) -> Value {
    let tool_calls = tool_calls_from_response(response);
    let mut content = Vec::new();
    if !response.content.is_empty() {
        content.push(json!({ "type": "text", "text": response.content }));
    }
    for call in &tool_calls {
        content.push(json!({
            "type": "tool_use",
            "id": call.id,
            "name": call.function.name,
            "input": serde_json::from_str::<Value>(&call.function.arguments)
                .unwrap_or_else(|_| json!({})),
        }));
    }
    json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": if tool_calls.is_empty() { "end_turn" } else { "tool_use" },
        "stop_sequence": Value::Null,
        "usage": {
            "input_tokens": response.input_tokens,
            "output_tokens": response.output_tokens,
        },
    })
}

/// Shape one buffered internal response as an OpenAI Responses object.
pub fn responses_response(id: &str, model: &str, created: u64, response: &ModelResponse) -> Value {
    let mut output = Vec::new();
    if !response.content.is_empty() {
        output.push(json!({
            "type": "message",
            "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
            "status": "completed",
            "role": "assistant",
            "content": [{ "type": "output_text", "text": response.content, "annotations": [] }],
        }));
    }
    for call in tool_calls_from_response(response) {
        output.push(json!({
            "type": "function_call",
            "id": format!("fc_{}", uuid::Uuid::new_v4().simple()),
            "call_id": call.id,
            "name": call.function.name,
            "arguments": call.function.arguments,
            "status": "completed",
        }));
    }
    json!({
        "id": id,
        "object": "response",
        "created_at": created,
        "status": "completed",
        "model": model,
        "output": output,
        "usage": {
            "input_tokens": response.input_tokens,
            "output_tokens": response.output_tokens,
            "total_tokens": response.input_tokens + response.output_tokens,
        },
    })
}

// ---------------------------------------------------------------------------
// Streaming egress: the framing both encoders write through.
// ---------------------------------------------------------------------------

fn sse(event: &str, data: Value) -> Event {
    Event::default()
        .event(event)
        .data(serde_json::to_string(&data).unwrap_or_default())
}
