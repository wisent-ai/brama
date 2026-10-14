//! What the OpenAI Responses event stream says.
//!
//! Same vendor as the chat wire, different protocol, and nothing about reading
//! it is shared: events are typed by name, a tool call is an output item keyed
//! by `output_index`, and the meter arrives with the stop reason on one
//! terminal `response.completed` or `response.incomplete` rather than trailing
//! along. Keeping it apart is what stops the older wire's habits from being
//! read into this one.

use serde_json::Value;

use super::event::{json_u32, tool_call_index, tool_call_start, StreamDelta, StreamItem};

/// Translate one OpenAI Responses event into neutral items.
pub(super) fn responses_items(event: Option<&str>, data: &str) -> (Vec<StreamItem>, bool) {
    let mut items = Vec::new();
    let kind = event
        .map(str::to_string)
        .or_else(|| {
            serde_json::from_str::<Value>(data)
                .ok()
                .and_then(|body| body.get("type").and_then(Value::as_str).map(str::to_string))
        })
        .unwrap_or_default();
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
    match kind.as_str() {
        "response.output_text.delta" => {
            if let Some(text) = body.get("delta").and_then(Value::as_str) {
                if !text.is_empty() {
                    items.push(StreamItem::Delta(StreamDelta::Text(text.to_string())));
                }
            }
        }
        "response.output_item.added" => {
            if body.pointer("/item/type").and_then(Value::as_str) == Some("function_call") {
                let id = match body.pointer("/item/call_id") {
                    Some(call_id) => Some(call_id),
                    None => body.pointer("/item/id"),
                };
                match tool_call_start(
                    "response.output_item.added",
                    body.get("output_index"),
                    id,
                    body.pointer("/item/name"),
                ) {
                    Ok(start) => items.push(StreamItem::Delta(start)),
                    Err(cause) => {
                        items.push(StreamItem::Failed(cause));
                        return (items, true);
                    }
                }
            }
        }
        "response.function_call_arguments.delta" => {
            if let Some(delta) = body.get("delta").and_then(Value::as_str) {
                if !delta.is_empty() {
                    let index = match tool_call_index(
                        "response.function_call_arguments.delta",
                        body.get("output_index"),
                    ) {
                        Ok(index) => index,
                        Err(cause) => {
                            items.push(StreamItem::Failed(cause));
                            return (items, true);
                        }
                    };
                    items.push(StreamItem::Delta(StreamDelta::ToolCallArguments {
                        index,
                        delta: delta.to_string(),
                    }));
                }
            }
        }
        "response.completed" | "response.incomplete" => {
            let response = body.get("response").cloned().unwrap_or(Value::Null);
            let usage = response.get("usage").cloned().unwrap_or(Value::Null);
            // Usage is passed on only when the provider stated both counts;
            // a stream without it is recorded as carrying no measurement.
            if let (Some(input_tokens), Some(output_tokens)) = (
                json_u32(usage.get("input_tokens")),
                json_u32(usage.get("output_tokens")),
            ) {
                items.push(StreamItem::Delta(StreamDelta::Usage {
                    input_tokens,
                    output_tokens,
                }));
            }
            let reason = if kind == "response.incomplete" {
                Some(
                    response
                        .pointer("/incomplete_details/reason")
                        .and_then(Value::as_str)
                        .map(|reason| format!("incomplete:{reason}"))
                        .unwrap_or_else(|| "incomplete".to_string()),
                )
            } else {
                Some("stop".to_string())
            };
            items.push(StreamItem::Delta(StreamDelta::Finish { reason }));
            return (items, true);
        }
        "response.failed" | "error" => {
            let message = body
                .pointer("/response/error/message")
                .or_else(|| body.pointer("/error/message"))
                .or_else(|| body.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("provider reported a stream error");
            items.push(StreamItem::Failed(message.to_string()));
            return (items, true);
        }
        _ => {}
    }
    (items, false)
}
