//! How one OpenAI Responses request becomes the call Brama routes.
//!
//! This format states a conversation as a flat list of typed input items --
//! a message, a `function_call`, a `function_call_output` -- where the
//! internal shape wants roles, and it offers a stored-conversation mode Brama
//! does not have. Reading it is kept apart from Anthropic's reading because
//! the two have nothing in common beyond the bounds the parent applies: the
//! same vendor's own two protocols already disagree about how a tool call is
//! written down, and one of them changing must not require rereading the
//! other.

use serde_json::{json, Value};

use crate::types::{Message, ModelRequest, Tool, ToolFunction};

use super::{validate, InboundCall};

/// One required text field of an input item or part, refused by its type and
/// field name when absent: an empty stand-in sent upstream a tool call with
/// no id or name, or a role the caller never stated.
fn item_text<'a>(item: &'a Value, kind: &str, field: &str) -> Result<&'a str, String> {
    item.get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("a `{kind}` item carries no `{field}` text"))
}

/// A message item's content in the internal shape: its text, or its parts
/// when an image is among them. A part this gateway cannot carry is refused,
/// since dropping it left the model answering without it.
fn message_content(content: &Value) -> Result<Value, String> {
    let parts = match content {
        Value::String(text) => return Ok(Value::String(text.clone())),
        Value::Array(parts) => parts,
        other => {
            return Err(format!(
                "a `message` item's content must be text or a list of parts, not {other}"
            ))
        }
    };
    let mut mapped = Vec::new();
    for part in parts {
        mapped.push(match part.get("type").and_then(Value::as_str) {
            Some("input_text") | Some("output_text") => {
                json!({ "type": "text", "text": item_text(part, "text part", "text")? })
            }
            Some("input_image") => json!({
                "type": "image_url",
                "image_url": { "url": item_text(part, "input_image", "image_url")? },
            }),
            _ => return Err(format!("a content part this gateway cannot carry: {part}")),
        });
    }
    let has_images = mapped
        .iter()
        .any(|part| part.get("type").and_then(Value::as_str) == Some("image_url"));
    if has_images {
        return Ok(Value::Array(mapped));
    }
    Ok(Value::String(
        mapped
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
    ))
}

/// One input item as internal messages: none for an item kind Brama keeps no
/// state for (reasoning and the like), refused when a field it needs is not
/// there.
fn input_item(item: &Value) -> Result<Option<Message>, String> {
    // The format reads an item without a type as a message.
    let kind = match item.get("type").and_then(Value::as_str) {
        Some(kind) => kind,
        None => "message",
    };
    let message = match kind {
        "message" => Message {
            role: item_text(item, "message", "role")?.to_string(),
            content: message_content(
                item.get("content")
                    .ok_or("a `message` item carries no `content`")?,
            )?,
            tool_call_id: None,
            name: None,
            tool_calls: None,
        },
        "function_call" => {
            let id = match item
                .get("call_id")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
            {
                Some(id) => id,
                None => return Err("a `function_call` item carries no `call_id` text".to_string()),
            };
            Message {
                role: "assistant".to_string(),
                content: Value::String(String::new()),
                tool_call_id: None,
                name: None,
                tool_calls: Some(vec![json!({
                    "id": id,
                    "type": "function",
                    "function": {
                        "name": item_text(item, "function_call", "name")?,
                        "arguments": item_text(item, "function_call", "arguments")?,
                    },
                })]),
            }
        }
        "function_call_output" => Message {
            role: "tool".to_string(),
            content: Value::String(item_text(item, "function_call_output", "output")?.to_string()),
            tool_call_id: Some(item_text(item, "function_call_output", "call_id")?.to_string()),
            name: None,
            tool_calls: None,
        },
        _ => return Ok(None),
    };
    Ok(Some(message))
}

/// Parse one OpenAI Responses request into the internal call shape.
///
/// `store`, `previous_response_id`, `reasoning`, `include` and non-function
/// tool types are accepted and dropped: Brama is stateless across calls and
/// the internal request has no field for them. `instructions` maps to the
/// system slot, matching how the buffered adapter already translates it back.
/// A field the format requires -- a message's role and content, a function
/// call's id, name and arguments, an output's call id and text, a function
/// tool's name -- is refused by position and name rather than sent blank.
pub fn responses_request(body: &[u8]) -> Result<InboundCall, String> {
    let raw: Value =
        serde_json::from_slice(body).map_err(|error| format!("invalid JSON: {error}"))?;
    let model = raw
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let max_tokens = raw
        .get("max_output_tokens")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    let temperature = raw.get("temperature").and_then(Value::as_f64);
    validate(&model, max_tokens, temperature)?;
    let system = raw
        .get("instructions")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let mut messages = Vec::new();
    match raw.get("input") {
        Some(Value::String(text)) => messages.push(Message {
            role: "user".to_string(),
            content: Value::String(text.clone()),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }),
        Some(Value::Array(items)) => {
            for (index, item) in items.iter().enumerate() {
                if let Some(message) =
                    input_item(item).map_err(|error| format!("input item {index}: {error}"))?
                {
                    messages.push(message);
                }
            }
        }
        Some(other) => {
            return Err(format!(
                "input must be text or a list of items, not {other}"
            ))
        }
        None => return Err("missing field `input`".to_string()),
    }
    if messages.is_empty() {
        return Err("input must not be empty".to_string());
    }
    let mut functions = Vec::new();
    for (index, tool) in raw
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, tool)| tool.get("type").and_then(Value::as_str) == Some("function"))
    {
        // The format leaves a description optional; none stated is the
        // internal shape's empty description.
        let description = match tool.get("description").and_then(Value::as_str) {
            Some(description) => description.to_string(),
            None => String::new(),
        };
        functions.push(Tool {
            tool_type: "function".to_string(),
            function: ToolFunction {
                name: item_text(tool, "function tool", "name")
                    .map_err(|error| format!("tool {index}: {error}"))?
                    .to_string(),
                description,
                parameters: tool.get("parameters").cloned(),
            },
        });
    }
    let tools = Some(functions).filter(|tools| !tools.is_empty());
    let tool_choice = match raw.get("tool_choice") {
        None => None,
        Some(Value::String(value)) => Some(json!(value)),
        Some(choice) if choice.get("type").and_then(Value::as_str) == Some("function") => {
            Some(json!({
                "type": "function",
                "function": { "name": item_text(choice, "function tool_choice", "name")? },
            }))
        }
        Some(choice) => {
            return Err(format!(
                "tool_choice {choice} is not a mode or a function this gateway carries"
            ))
        }
    };
    super::validate_history_tool_calls(&messages)?;
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
            response_schema: None,
        },
        stream: raw
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
    })
}
