//! What the OpenAI Responses event stream says.
//!
//! Same vendor as the chat wire, different protocol, and nothing about reading
//! it is shared: events are typed by name, a tool call is an output item keyed
//! by `output_index`, and the meter arrives with the stop reason on one
//! terminal `response.completed` or `response.incomplete` rather than trailing
//! along. Keeping it apart is what stops the older wire's habits from being
//! read into this one.

use serde_json::Value;

use super::event::{json_u32, StreamDelta, StreamItem};

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
    let body: Value = match serde_json::from_str(data) {
        Ok(body) => body,
        Err(_) => return (items, false),
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
            let index = json_u32(body.get("output_index")).unwrap_or_default();
            let item = body.get("item").cloned().unwrap_or(Value::Null);
            if item.get("type").and_then(Value::as_str) == Some("function_call") {
                items.push(StreamItem::Delta(StreamDelta::ToolCallStart {
                    index,
                    id: item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                }));
            }
        }
        "response.function_call_arguments.delta" => {
            let index = json_u32(body.get("output_index")).unwrap_or_default();
            if let Some(delta) = body.get("delta").and_then(Value::as_str) {
                if !delta.is_empty() {
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
            items.push(StreamItem::Delta(StreamDelta::Usage {
                input_tokens: json_u32(usage.get("input_tokens")).unwrap_or_default(),
                output_tokens: json_u32(usage.get("output_tokens")).unwrap_or_default(),
            }));
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
            items.push(StreamItem::Failed(message.chars().take(200).collect()));
            return (items, true);
        }
        _ => {}
    }
    (items, false)
}
