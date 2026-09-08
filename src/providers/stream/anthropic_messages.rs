//! What the Anthropic messages event stream says.
//!
//! This is the only wire here whose meter is split across the stream -- input
//! tokens announced on `message_start`, output tokens only on `message_delta`
//! -- so it is also the only one that has to remember anything between events.
//! That memory and the events that fill it are one subject and are kept
//! together, which is why the pump hands this reader a state of its own and
//! nothing else in the crate has to know what is inside it.

use serde_json::Value;

use super::event::{json_u32, StreamDelta, StreamItem};

/// Mutable state for the Anthropic messages wire: the input meter arrives at
/// the start of the stream and the output meter only at its end.
///
/// A stream begins having been told no input tokens, which is what an unread
/// meter means and what `Default` gives.
#[derive(Default)]
pub(super) struct AnthropicState {
    input_tokens: u32,
}

/// Translate one Anthropic messages event into neutral items.
pub(super) fn anthropic_items(
    event: Option<&str>,
    data: &str,
    state: &mut AnthropicState,
) -> (Vec<StreamItem>, bool) {
    let mut items = Vec::new();
    let body: Value = match serde_json::from_str(data) {
        Ok(body) => body,
        Err(_) => return (items, false),
    };
    match event.unwrap_or_else(|| body.get("type").and_then(Value::as_str).unwrap_or("")) {
        "message_start" => {
            state.input_tokens =
                json_u32(body.pointer("/message/usage/input_tokens")).unwrap_or_default();
        }
        "content_block_start" => {
            let index = json_u32(body.get("index")).unwrap_or_default();
            let block = body.get("content_block").cloned().unwrap_or(Value::Null);
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                items.push(StreamItem::Delta(StreamDelta::ToolCallStart {
                    index,
                    id: block
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                }));
            }
        }
        "content_block_delta" => {
            let index = json_u32(body.get("index")).unwrap_or_default();
            let delta = body.get("delta").cloned().unwrap_or(Value::Null);
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => {
                    if let Some(text) = delta.get("text").and_then(Value::as_str) {
                        if !text.is_empty() {
                            items.push(StreamItem::Delta(StreamDelta::Text(text.to_string())));
                        }
                    }
                }
                Some("input_json_delta") => {
                    if let Some(partial) = delta.get("partial_json").and_then(Value::as_str) {
                        if !partial.is_empty() {
                            items.push(StreamItem::Delta(StreamDelta::ToolCallArguments {
                                index,
                                delta: partial.to_string(),
                            }));
                        }
                    }
                }
                _ => {}
            }
        }
        "message_delta" => {
            let output = json_u32(body.pointer("/usage/output_tokens")).unwrap_or_default();
            if state.input_tokens > 0 || output > 0 {
                items.push(StreamItem::Delta(StreamDelta::Usage {
                    input_tokens: state.input_tokens,
                    output_tokens: output,
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
            let message = body
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("provider reported a stream error");
            items.push(StreamItem::Failed(message.chars().take(200).collect()));
            return (items, true);
        }
        _ => {}
    }
    (items, false)
}
