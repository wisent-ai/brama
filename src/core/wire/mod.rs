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
/// (input, output) token meters when it stated them, `None` when it stated
/// none, and whether the generation failed.
///
/// The encoders live here, but process statistics belong to the HTTP layer
/// that owns them, so the layer hands each stream a closure instead of this
/// module reaching for a static it does not own.
pub type StreamAccounting = Box<dyn FnMut(Option<(u32, u32)>, bool) + Send>;

/// One parsed inbound call, format already erased.
pub struct InboundCall {
    pub model: String,
    pub request: ModelRequest,
    pub stream: bool,
}

/// Shared inbound bounds for all three formats a caller may speak, so the
/// answer does not depend on which one it uses.
pub(crate) fn validate(
    model: &str,
    max_tokens: Option<u32>,
    temperature: Option<f64>,
) -> Result<(), String> {
    use crate::core::server::{MAX_TEMPERATURE, MIN_TEMPERATURE};
    if model.trim().is_empty() {
        return Err("missing field `model`".to_string());
    }
    if max_tokens.is_some_and(|tokens| std::num::NonZeroU32::new(tokens).is_none()) {
        return Err("max_tokens must be at least one when it is set".to_string());
    }
    if temperature.is_some_and(|temperature| {
        !temperature.is_finite() || !(MIN_TEMPERATURE..=MAX_TEMPERATURE).contains(&temperature)
    }) {
        return Err(format!(
            "temperature must be finite and between {MIN_TEMPERATURE} and {MAX_TEMPERATURE}"
        ));
    }
    Ok(())
}

/// Every tool call a caller's history carries, as each provider wire needs
/// it: an `id`, a `function.name` and `function.arguments` that are JSON text;
/// and every tool result, which must name by `tool_call_id` a call an earlier
/// message carries. Checked once at the door for all three formats, so no
/// provider translation stands in a call named `tool`, empty arguments, or a
/// result answering no call, for one it cannot read.
pub fn validate_history_tool_calls(messages: &[crate::types::Message]) -> Result<(), String> {
    let mut called = std::collections::BTreeSet::new();
    for (position, message) in messages.iter().enumerate() {
        if message.role == "tool" {
            match message.tool_call_id.as_deref() {
                None => {
                    return Err(format!(
                        "message {position}: a tool result carries no `tool_call_id`"
                    ))
                }
                Some(id) if !called.contains(id) => {
                    return Err(format!(
                        "message {position}: a tool result answers `{id}`, which no earlier tool call carries"
                    ))
                }
                Some(_) => {}
            }
        }
        for call in message.tool_calls.iter().flatten() {
            let text = |pointer: &str| call.pointer(pointer).and_then(Value::as_str);
            let refused =
                |what: &str| Err(format!("message {position}: a tool call {what}: {call}"));
            let Some(id) = text("/id") else {
                return refused("carries no `id` text");
            };
            if text("/function/name").is_none() {
                return refused("carries no `function.name` text");
            }
            match text("/function/arguments") {
                None => return refused("carries no `function.arguments` text"),
                Some(arguments) => {
                    if let Err(error) = serde_json::from_str::<Value>(arguments) {
                        return Err(format!(
                            "message {position}: a tool call's `function.arguments` are not JSON ({error}): {call}"
                        ));
                    }
                }
            }
            called.insert(id.to_string());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Buffered egress.
// ---------------------------------------------------------------------------

/// The tool calls a response carries; a response that made none carries an
/// empty list, which is what `None` states.
fn tool_calls_from_response(response: &ModelResponse) -> &[ToolCall] {
    match response.tool_calls.as_deref() {
        Some(calls) => calls,
        None => &[],
    }
}

/// Shape one buffered internal response as an Anthropic message.
///
/// `stop_reason` is derived, not reported: the internal response does not
/// carry the provider's reason, so a tool call reads as `tool_use` and
/// anything else as `end_turn` -- the two readings a caller can act on.
///
/// A tool call whose arguments are not JSON is refused with the tool's name
/// and the arguments as the model wrote them: reading them as `{}` handed
/// the caller a call to run with none of the arguments the model meant.
/// Arguments that are empty are a call without arguments.
pub fn anthropic_response(
    id: &str,
    model: &str,
    response: &ModelResponse,
) -> Result<Value, String> {
    let tool_calls = tool_calls_from_response(response);
    let mut content = Vec::new();
    if !response.content.is_empty() {
        content.push(json!({ "type": "text", "text": response.content }));
    }
    for call in tool_calls {
        let arguments = call.function.arguments.trim();
        let input = if arguments.is_empty() {
            json!({})
        } else {
            serde_json::from_str::<Value>(arguments).map_err(|error| {
                format!(
                    "the model called {} with arguments that are not JSON ({error}): {arguments}",
                    call.function.name
                )
            })?
        };
        content.push(json!({
            "type": "tool_use",
            "id": call.id,
            "name": call.function.name,
            "input": input,
        }));
    }
    let mut message = json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": if tool_calls.is_empty() { "end_turn" } else { "tool_use" },
        "stop_sequence": Value::Null,
    });
    // The provider's stated meter, absent when it stated none, never zeros.
    if let Some(usage) = response.usage {
        message["usage"] = json!({
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
        });
    }
    Ok(message)
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
    let mut document = json!({
        "id": id,
        "object": "response",
        "created_at": created,
        "status": "completed",
        "model": model,
        "output": output,
    });
    // The provider's stated meter, absent when it stated none, never zeros.
    if let Some(usage) = response.usage {
        document["usage"] = json!({
            "input_tokens": usage.input_tokens,
            "output_tokens": usage.output_tokens,
            "total_tokens": usage.total(),
        });
    }
    document
}

// ---------------------------------------------------------------------------
// Streaming egress: the framing both encoders write through.
// ---------------------------------------------------------------------------

fn sse(event: &str, data: Value) -> Event {
    // A JSON value always writes; its Display is that writing.
    Event::default().event(event).data(data.to_string())
}
