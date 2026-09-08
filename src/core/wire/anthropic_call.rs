//! How one Anthropic Messages request becomes the call Brama routes.
//!
//! Of the three formats that reach this crate, Anthropic's request is the
//! least like the internal shape: a message's content is a block list rather
//! than a string, an image rides inside it as base64, and a tool result is a
//! block in a `user` message where the internal shape wants a `tool` message
//! of its own. Reading that is a subject in itself, so it is kept here alone:
//! the bounds every inbound request is held to stay in the parent, and this
//! module only decides what Anthropic's fields mean and which of them the
//! internal request has no home for.

use serde_json::{json, Value};

use crate::types::{Message, ModelRequest, Tool, ToolFunction};

use super::{default_temperature, validate, InboundCall};

/// Turn one Anthropic content block list into OpenAI-shaped message fields.
///
/// Returns `(content, tool_calls, tool_messages)`: the text-and-image content
/// of this message, the tool calls an assistant made in it, and the tool
/// results it carries -- Anthropic puts `tool_result` blocks inside a `user`
/// message, while the internal shape gives each result its own `tool` role
/// message, appended after this one.
fn anthropic_content_in(content: &Value) -> (Value, Option<Vec<Value>>, Vec<Message>) {
    let mut text = String::new();
    let mut parts: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut tool_messages: Vec<Message> = Vec::new();
    let blocks: Vec<Value> = match content {
        Value::String(plain) => {
            return (Value::String(plain.clone()), None, Vec::new());
        }
        Value::Array(blocks) => blocks.clone(),
        _ => return (Value::String(String::new()), None, Vec::new()),
    };
    for block in &blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let value = block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                text.push_str(value);
                parts.push(json!({ "type": "text", "text": value }));
            }
            Some("image") => {
                let source = block.get("source").cloned().unwrap_or(Value::Null);
                if source.get("type").and_then(Value::as_str) == Some("base64") {
                    let media_type = source
                        .get("media_type")
                        .and_then(Value::as_str)
                        .unwrap_or("image/png");
                    let data = source
                        .get("data")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    parts.push(json!({
                        "type": "image_url",
                        "image_url": { "url": format!("data:{media_type};base64,{data}") },
                    }));
                }
            }
            Some("tool_use") => {
                tool_calls.push(json!({
                    "id": block.get("id").and_then(Value::as_str).unwrap_or_default(),
                    "type": "function",
                    "function": {
                        "name": block.get("name").and_then(Value::as_str).unwrap_or_default(),
                        "arguments": block
                            .get("input")
                            .map(|input| serde_json::to_string(input).unwrap_or_default())
                            .unwrap_or_default(),
                    },
                }));
            }
            Some("tool_result") => {
                let result_text = match block.get("content") {
                    Some(Value::String(plain)) => plain.clone(),
                    Some(Value::Array(blocks)) => blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(""),
                    _ => String::new(),
                };
                tool_messages.push(Message {
                    role: "tool".to_string(),
                    content: Value::String(result_text),
                    tool_call_id: block
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    name: None,
                    tool_calls: None,
                });
            }
            _ => {}
        }
    }
    let has_images = parts
        .iter()
        .any(|part| part.get("type").and_then(Value::as_str) == Some("image_url"));
    let content = if has_images {
        Value::Array(parts)
    } else {
        Value::String(text)
    };
    let tool_calls = if tool_calls.is_empty() {
        None
    } else {
        Some(tool_calls)
    };
    (content, tool_calls, tool_messages)
}

/// Parse one Anthropic Messages request into the internal call shape.
///
/// `max_tokens` is required by the format and bounded by the same contract the
/// chat endpoint enforces. `metadata`, `stop_sequences`, `cache_control` and
/// `thinking` blocks are accepted and dropped: the internal request has no
/// field that could hold them honestly.
pub fn anthropic_request(body: &[u8]) -> Result<InboundCall, String> {
    let raw: Value =
        serde_json::from_slice(body).map_err(|error| format!("invalid JSON: {error}"))?;
    let model = raw
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let max_tokens = raw
        .get("max_tokens")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or_default();
    let temperature = raw
        .get("temperature")
        .and_then(Value::as_f64)
        .unwrap_or_else(default_temperature);
    validate(&model, max_tokens, temperature)?;
    let system = match raw.get("system") {
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(blocks)) => {
            let joined = blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            if joined.is_empty() {
                None
            } else {
                Some(joined)
            }
        }
        _ => None,
    };
    let mut messages = Vec::new();
    for raw_message in raw
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let role = raw_message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("user")
            .to_string();
        let (content, tool_calls, tool_messages) =
            anthropic_content_in(raw_message.get("content").unwrap_or(&Value::Null));
        messages.push(Message {
            role,
            content,
            tool_call_id: None,
            name: None,
            tool_calls,
        });
        messages.extend(tool_messages);
    }
    if messages.is_empty() {
        return Err("messages must not be empty".to_string());
    }
    let tools = raw
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter(|tool| tool.get("name").and_then(Value::as_str).is_some())
                .map(|tool| Tool {
                    tool_type: "function".to_string(),
                    function: ToolFunction {
                        name: tool
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        description: tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        parameters: tool.get("input_schema").cloned(),
                    },
                })
                .collect::<Vec<_>>()
        })
        .filter(|tools: &Vec<Tool>| !tools.is_empty());
    let tool_choice = raw.get("tool_choice").and_then(|choice| {
        match choice.get("type").and_then(Value::as_str) {
            Some("auto") => Some(json!("auto")),
            Some("any") => Some(json!("required")),
            Some("none") => Some(json!("none")),
            Some("tool") => Some(json!({
                "type": "function",
                "function": { "name": choice.get("name").and_then(Value::as_str).unwrap_or_default() },
            })),
            _ => None,
        }
    });
    Ok(InboundCall {
        model,
        request: ModelRequest {
            messages,
            model: String::new(),
            max_tokens,
            temperature,
            system,
            tools,
            tool_choice,
            billing_target: None,
        },
        stream: raw
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
    })
}
