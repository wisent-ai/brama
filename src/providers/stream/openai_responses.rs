//! What the OpenAI Responses event stream says.
//!
//! Same vendor as the chat wire, different protocol, and nothing about reading
//! it is shared: events are typed by name, a tool call is an output item keyed
//! by `output_index`, and the meter arrives with the stop reason on one
//! terminal `response.completed` or `response.incomplete` rather than trailing
//! along. Keeping it apart is what stops the older wire's habits from being
//! read into this one.

use serde_json::Value;

use super::event::{
    json_u32, stream_error, tool_call_index, tool_call_start, StopReason, StreamDelta, StreamItem,
};

/// A Responses `incomplete_details.reason` as a stop reason: the values the
/// API documents (https://platform.openai.com/docs/api-reference/responses/object),
/// and any other kept as the provider stated it.
fn incomplete_reason(stated: Option<&str>) -> StopReason {
    match stated {
        Some("max_output_tokens") => StopReason::MaxTokens,
        Some("content_filter") => StopReason::ContentFilter,
        other => StopReason::Incomplete(other.map(str::to_string)),
    }
}

/// Translate one OpenAI Responses event into neutral items.
pub(super) fn responses_items(event: Option<&str>, data: &str) -> (Vec<StreamItem>, bool) {
    let mut items = Vec::new();
    // An event that is not JSON ends the stream with the event named, so what
    // it carries never vanishes from an answer that still finishes as complete.
    let body: Value = match serde_json::from_str(data) {
        Ok(body) => body,
        Err(error) => {
            items.push(StreamItem::Failed(format!(
                "provider stream event is not JSON ({error}): {data}"
            )));
            return (items, true);
        }
    };
    // The SSE event name, or the type the body states. An event that names
    // neither carries nothing this translator reads, as an event of a type
    // it does not read carries nothing.
    let kind = match event {
        Some(event) => event,
        None => match body.get("type").and_then(Value::as_str) {
            Some(kind) => kind,
            None => return (items, false),
        },
    };
    match kind {
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
            // Usage is passed on only when the provider stated both counts;
            // a stream without it is recorded as carrying no measurement.
            match (
                json_u32(
                    body.pointer("/response/usage/input_tokens"),
                    "response.usage.input_tokens",
                ),
                json_u32(
                    body.pointer("/response/usage/output_tokens"),
                    "response.usage.output_tokens",
                ),
            ) {
                (Ok(Some(input_tokens)), Ok(Some(output_tokens))) => {
                    items.push(StreamItem::Delta(StreamDelta::Usage {
                        input_tokens,
                        output_tokens,
                    }));
                }
                (Err(cause), _) | (_, Err(cause)) => {
                    items.push(StreamItem::Failed(cause));
                    return (items, true);
                }
                _ => {}
            }
            let reason = if kind == "response.incomplete" {
                incomplete_reason(
                    body.pointer("/response/incomplete_details/reason")
                        .and_then(Value::as_str),
                )
            } else if body
                .pointer("/response/output")
                .and_then(Value::as_array)
                .is_some_and(|output| {
                    output.iter().any(|item| {
                        item.get("type").and_then(Value::as_str) == Some("function_call")
                    })
                })
            {
                // A completed response whose output holds a function call
                // stopped to have it run.
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            };
            items.push(StreamItem::Delta(StreamDelta::Finish { reason }));
            return (items, true);
        }
        "response.failed" | "error" => {
            let message = body
                .pointer("/response/error/message")
                .or_else(|| body.pointer("/error/message"))
                .or_else(|| body.get("message"));
            items.push(StreamItem::Failed(stream_error(message, &body)));
            return (items, true);
        }
        _ => {}
    }
    (items, false)
}
