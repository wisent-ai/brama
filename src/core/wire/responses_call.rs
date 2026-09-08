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

use super::{default_max_tokens, default_temperature, validate, InboundCall};

/// Parse one OpenAI Responses request into the internal call shape.
///
/// `store`, `previous_response_id`, `reasoning`, `include` and non-function
/// tool types are accepted and dropped: Brama is stateless across calls and
/// the internal request has no field for them. `instructions` maps to the
/// system slot, matching how the buffered adapter already translates it back.
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
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(default_max_tokens);
    let temperature = raw
        .get("temperature")
        .and_then(Value::as_f64)
        .unwrap_or_else(default_temperature);
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
            for item in items {
                match item
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("message")
                {
                    "message" => {
                        let role = item
                            .get("role")
                            .and_then(Value::as_str)
                            .unwrap_or("user")
                            .to_string();
                        let content = match item.get("content") {
                            Some(Value::String(text)) => Value::String(text.clone()),
                            Some(Value::Array(parts)) => {
                                let mapped: Vec<Value> =
                                    parts
                                        .iter()
                                        .filter_map(|part| {
                                            match part.get("type").and_then(Value::as_str) {
                                        Some("input_text") | Some("output_text") => part
                                            .get("text")
                                            .and_then(Value::as_str)
                                            .map(|text| json!({ "type": "text", "text": text })),
                                        Some("input_image") => part
                                            .get("image_url")
                                            .and_then(Value::as_str)
                                            .map(|url| json!({
                                                "type": "image_url",
                                                "image_url": { "url": url },
                                            })),
                                        _ => None,
                                    }
                                        })
                                        .collect();
                                if mapped.iter().any(|part| {
                                    part.get("type").and_then(Value::as_str) == Some("image_url")
                                }) {
                                    Value::Array(mapped)
                                } else {
                                    Value::String(
                                        mapped
                                            .iter()
                                            .filter_map(|part| {
                                                part.get("text").and_then(Value::as_str)
                                            })
                                            .collect::<Vec<_>>()
                                            .join(""),
                                    )
                                }
                            }
                            _ => Value::String(String::new()),
                        };
                        messages.push(Message {
                            role,
                            content,
                            tool_call_id: None,
                            name: None,
                            tool_calls: None,
                        });
                    }
                    "function_call" => {
                        messages.push(Message {
                            role: "assistant".to_string(),
                            content: Value::String(String::new()),
                            tool_call_id: None,
                            name: None,
                            tool_calls: Some(vec![json!({
                                "id": item.get("call_id").or_else(|| item.get("id"))
                                    .and_then(Value::as_str).unwrap_or_default(),
                                "type": "function",
                                "function": {
                                    "name": item.get("name").and_then(Value::as_str).unwrap_or_default(),
                                    "arguments": item.get("arguments").and_then(Value::as_str).unwrap_or_default(),
                                },
                            })]),
                        });
                    }
                    "function_call_output" => {
                        messages.push(Message {
                            role: "tool".to_string(),
                            content: Value::String(
                                item.get("output")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                            ),
                            tool_call_id: item
                                .get("call_id")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            name: None,
                            tool_calls: None,
                        });
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    if messages.is_empty() {
        return Err("input must not be empty".to_string());
    }
    let tools = raw
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter(|tool| tool.get("type").and_then(Value::as_str) == Some("function"))
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
                        parameters: tool.get("parameters").cloned(),
                    },
                })
                .collect::<Vec<_>>()
        })
        .filter(|tools: &Vec<Tool>| !tools.is_empty());
    let tool_choice = raw.get("tool_choice").and_then(|choice| match choice {
        Value::String(value) => Some(json!(value)),
        Value::Object(_) if choice.get("type").and_then(Value::as_str) == Some("function") => {
            Some(json!({
                "type": "function",
                "function": { "name": choice.get("name").and_then(Value::as_str).unwrap_or_default() },
            }))
        }
        _ => None,
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
