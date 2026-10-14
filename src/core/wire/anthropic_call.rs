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

use super::{validate, InboundCall};

/// One required text field of a content block, refused by block type and
/// field name when absent: an empty stand-in sent upstream a tool call with
/// no id or name, or an image whose bytes were nothing.
fn block_text<'a>(block: &'a Value, kind: &str, field: &str) -> Result<&'a str, String> {
    block
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("a `{kind}` content block carries no `{field}` text"))
}

/// What one Anthropic content block list becomes in the internal shape.
struct ContentIn {
    content: Value,
    tool_calls: Option<Vec<Value>>,
    tool_messages: Vec<Message>,
}

/// Turn one Anthropic content block list into OpenAI-shaped message fields:
/// the text-and-image content of this message, the tool calls an assistant
/// made in it, and the tool results it carries -- Anthropic puts
/// `tool_result` blocks inside a `user` message, while the internal shape
/// gives each result its own `tool` role message, appended after this one.
fn anthropic_content_in(content: &Value) -> Result<ContentIn, String> {
    let mut text = String::new();
    let mut parts: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut tool_messages: Vec<Message> = Vec::new();
    let blocks = match content {
        Value::String(plain) => {
            return Ok(ContentIn {
                content: Value::String(plain.clone()),
                tool_calls: None,
                tool_messages: Vec::new(),
            });
        }
        Value::Array(blocks) => blocks,
        other => {
            return Err(format!(
                "a message's content must be text or a list of content blocks, not {other}"
            ))
        }
    };
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let value = block_text(block, "text", "text")?;
                text.push_str(value);
                parts.push(json!({ "type": "text", "text": value }));
            }
            Some("image") => {
                let source = block
                    .get("source")
                    .ok_or("an `image` content block carries no `source`")?;
                // Both sources Anthropic defines become the one URL the
                // internal shape carries; any other is refused, since
                // dropping the image left the model answering without it.
                let url = match source.get("type").and_then(Value::as_str) {
                    Some("base64") => format!(
                        "data:{};base64,{}",
                        block_text(source, "base64 image source", "media_type")?,
                        block_text(source, "base64 image source", "data")?
                    ),
                    Some("url") => block_text(source, "url image source", "url")?.to_string(),
                    _ => {
                        return Err(format!(
                            "an `image` content block's source must be base64 data or a url, not {source}"
                        ))
                    }
                };
                parts.push(json!({ "type": "image_url", "image_url": { "url": url } }));
            }
            Some("tool_use") => {
                let id = block_text(block, "tool_use", "id")?;
                let name = block_text(block, "tool_use", "name")?;
                let input = block
                    .get("input")
                    .ok_or("a `tool_use` content block carries no `input`")?;
                tool_calls.push(json!({
                    "id": id,
                    "type": "function",
                    "function": { "name": name, "arguments": input.to_string() },
                }));
            }
            Some("tool_result") => {
                // A result without content is the format's empty result.
                let result_text = match block.get("content") {
                    None => String::new(),
                    Some(Value::String(plain)) => plain.clone(),
                    Some(Value::Array(blocks)) => blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(""),
                    Some(other) => {
                        return Err(format!(
                            "a `tool_result` content block's content must be text or blocks, not {other}"
                        ))
                    }
                };
                tool_messages.push(Message {
                    role: "tool".to_string(),
                    content: Value::String(result_text),
                    tool_call_id: Some(
                        block_text(block, "tool_result", "tool_use_id")?.to_string(),
                    ),
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
    Ok(ContentIn {
        content,
        tool_calls,
        tool_messages,
    })
}

/// Parse one Anthropic Messages request into the internal call shape.
///
/// `max_tokens` is required by the format and must be at least one; no other
/// bound is put on it. `metadata`, `stop_sequences`, `cache_control` and
/// `thinking` blocks are accepted and dropped: the internal request has no
/// field that could hold them honestly. A field the format requires --
/// a message's role and content, a tool's name, a tool call's id, name and
/// input, a tool result's tool_use_id, an image's source -- is refused by
/// position and name rather than sent upstream empty or guessed.
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
        .ok_or_else(|| {
            "missing field `max_tokens`: the Anthropic Messages format requires it".to_string()
        })?;
    let temperature = raw.get("temperature").and_then(Value::as_f64);
    validate(&model, Some(max_tokens), temperature)?;
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
    let raw_messages = raw
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("missing field `messages`: the Anthropic Messages format requires a list")?;
    let mut messages = Vec::new();
    for (index, raw_message) in raw_messages.iter().enumerate() {
        // A message without a role was sent as the user's; one without
        // content as an empty one. Both are refused by position instead.
        let role = raw_message
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("message {index} names no `role`"))?
            .to_string();
        let content = raw_message
            .get("content")
            .ok_or_else(|| format!("message {index} carries no `content`"))?;
        let read =
            anthropic_content_in(content).map_err(|error| format!("message {index}: {error}"))?;
        messages.push(Message {
            role,
            content: read.content,
            tool_call_id: None,
            name: None,
            tool_calls: read.tool_calls,
        });
        messages.extend(read.tool_messages);
    }
    if messages.is_empty() {
        return Err("messages must not be empty".to_string());
    }
    let tools = match raw.get("tools").and_then(Value::as_array) {
        None => None,
        Some(declared) => {
            let mut tools = Vec::new();
            for (index, tool) in declared.iter().enumerate() {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("tool {index} names no `name`"))?;
                // The format leaves a description optional; none stated is
                // the internal shape's empty description.
                let description = match tool.get("description").and_then(Value::as_str) {
                    Some(description) => description.to_string(),
                    None => String::new(),
                };
                tools.push(Tool {
                    tool_type: "function".to_string(),
                    function: ToolFunction {
                        name: name.to_string(),
                        description,
                        parameters: tool.get("input_schema").cloned(),
                    },
                });
            }
            Some(tools).filter(|tools: &Vec<Tool>| !tools.is_empty())
        }
    };
    let tool_choice = match raw.get("tool_choice") {
        None => None,
        Some(choice) => Some(match choice.get("type").and_then(Value::as_str) {
            Some("auto") => json!("auto"),
            Some("any") => json!("required"),
            Some("none") => json!("none"),
            Some("tool") => json!({
                "type": "function",
                "function": {
                    "name": choice
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or("a `tool` tool_choice names no `name`")?,
                },
            }),
            _ => {
                return Err(format!(
                    "tool_choice {choice} is not auto, any, none or tool"
                ))
            }
        }),
    };
    super::validate_history_tool_calls(&messages)?;
    Ok(InboundCall {
        model,
        request: ModelRequest {
            messages,
            model: String::new(),
            max_tokens: Some(max_tokens),
            temperature,
            system,
            tools,
            tool_choice,
            billing_target: None,
            response_schema: None,
        },
        stream: raw
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
    })
}
