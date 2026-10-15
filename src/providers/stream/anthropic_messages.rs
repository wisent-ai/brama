//! What the Anthropic messages event stream says.
//!
//! This is the only wire here whose meter is split across the stream -- input
//! tokens announced on `message_start`, output tokens only on `message_delta`
//! -- so it is also the only one that has to remember anything between events.
//! That memory and the events that fill it are one subject and are kept
//! together, which is why the pump hands this reader a state of its own and
//! nothing else in the crate has to know what is inside it.

use serde_json::Value;

use super::event::{
    json_u32, stream_error, tool_call_index, tool_call_start, StreamDelta, StreamItem,
};

/// Mutable state for the Anthropic messages wire: the input meter arrives at
/// the start of the stream and the output meter only at its end.
///
/// A stream begins having been told no input meter. `None` is "not stated",
/// never zero tokens: a usage line is sent only when both meters were stated,
/// so a stream that did not state them is recorded as carrying no measurement.
#[derive(Default)]
pub(super) struct AnthropicState {
    input_tokens: Option<u32>,
}

/// Translate one Anthropic messages event into neutral items.
pub(super) fn anthropic_items(
    event: Option<&str>,
    data: &str,
    state: &mut AnthropicState,
) -> (Vec<StreamItem>, bool) {
    let mut items = Vec::new();
    // An event that is not JSON ends the stream with the event named: dropped,
    // what it carried vanished from an answer that still finished as complete.
    let body: Value = match serde_json::from_str(data) {
        Ok(body) => body,
        Err(error) => {
            items.push(StreamItem::Failed(format!(
                "provider stream event is not JSON ({error}): {data}"
            )));
            return (items, true);
        }
    };
    // Every event names its kind, as the SSE event or the body's type. One
    // that names neither cannot be read, and ignoring it would drop what it
    // carried from an answer that still finished as complete.
    let kind = match (event, body.get("type").and_then(Value::as_str)) {
        (Some(kind), _) | (None, Some(kind)) => kind,
        (None, None) => {
            items.push(StreamItem::Failed(format!(
                "provider stream event names no type: {data}"
            )));
            return (items, true);
        }
    };
    match kind {
        "message_start" => {
            state.input_tokens = json_u32(body.pointer("/message/usage/input_tokens"));
        }
        "content_block_start" => {
            if body.pointer("/content_block/type").and_then(Value::as_str) == Some("tool_use") {
                match tool_call_start(
                    "content_block_start",
                    body.get("index"),
                    body.pointer("/content_block/id"),
                    body.pointer("/content_block/name"),
                ) {
                    Ok(start) => items.push(StreamItem::Delta(start)),
                    Err(cause) => {
                        items.push(StreamItem::Failed(cause));
                        return (items, true);
                    }
                }
            }
        }
        "content_block_delta" => match body.pointer("/delta/type").and_then(Value::as_str) {
            Some("text_delta") => {
                if let Some(text) = body.pointer("/delta/text").and_then(Value::as_str) {
                    if !text.is_empty() {
                        items.push(StreamItem::Delta(StreamDelta::Text(text.to_string())));
                    }
                }
            }
            Some("input_json_delta") => {
                if let Some(partial) = body.pointer("/delta/partial_json").and_then(Value::as_str) {
                    if !partial.is_empty() {
                        let index = match tool_call_index("content_block_delta", body.get("index"))
                        {
                            Ok(index) => index,
                            Err(cause) => {
                                items.push(StreamItem::Failed(cause));
                                return (items, true);
                            }
                        };
                        items.push(StreamItem::Delta(StreamDelta::ToolCallArguments {
                            index,
                            delta: partial.to_string(),
                        }));
                    }
                }
            }
            _ => {}
        },
        "message_delta" => {
            if let (Some(input_tokens), Some(output_tokens)) = (
                state.input_tokens,
                json_u32(body.pointer("/usage/output_tokens")),
            ) {
                items.push(StreamItem::Delta(StreamDelta::Usage {
                    input_tokens,
                    output_tokens,
                }));
            }
            if let Some(reason) = body.pointer("/delta/stop_reason").and_then(Value::as_str) {
                items.push(StreamItem::Delta(StreamDelta::Finish {
                    reason: Some(reason.to_string()),
                }));
            }
        }
        "message_stop" => return (items, true),
        "error" => {
            items.push(StreamItem::Failed(stream_error(
                body.pointer("/error/message"),
                &body,
            )));
            return (items, true);
        }
        _ => {}
    }
    (items, false)
}
