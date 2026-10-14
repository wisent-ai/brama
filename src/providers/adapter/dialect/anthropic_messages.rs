//! What a conversation, its tools, a tool choice and an answer look like in the
//! Anthropic Messages dialect.

use serde_json::{json, Value};

use super::named_tool_choice;
use crate::providers::adapter::call::outcome::refusal::attempted_failure;
use crate::providers::adapter::stated_usage;
use crate::types::{Message, ModelRequest, ModelResponse, ProviderRefusal, Refusal, ToolCall};

pub(in crate::providers::adapter) fn anthropic_tool_choice(choice: &Value) -> Option<Value> {
    if let Some(name) = named_tool_choice(choice) {
        return Some(json!({"type": "tool", "name": name}));
    }
    match choice.as_str() {
        Some("auto") => Some(json!({"type": "auto"})),
        Some("required") => Some(json!({"type": "any"})),
        Some("none") => None,
        _ => None,
    }
}

fn anthropic_content(message: &Message) -> Value {
    if message.role == "tool" {
        return json!([{
            "type": "tool_result",
            "tool_use_id": message.tool_call_id,
            "content": message.content_text(),
        }]);
    }
    if let Some(calls) = &message.tool_calls {
        let mut blocks = Vec::new();
        let text = message.content_text();
        if !text.is_empty() {
            blocks.push(json!({"type": "text", "text": text}));
        }
        // Every history tool call was checked at the door for an id, a
        // function name and JSON arguments (wire::validate_history_tool_calls),
        // so each is carried as stated, with no stand-in for a missing part.
        for call in calls {
            let input = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|value| serde_json::from_str::<Value>(value).ok());
            blocks.push(json!({
                "type": "tool_use",
                "id": call.get("id"),
                "name": call.pointer("/function/name"),
                "input": input,
            }));
        }
        return Value::Array(blocks);
    }
    match &message.content {
        Value::Array(parts) => Value::Array(parts.iter().map(anthropic_block).collect()),
        content => content.clone(),
    }
}

fn anthropic_block(part: &Value) -> Value {
    if part.get("type").and_then(Value::as_str) != Some("image_url") {
        return part.clone();
    }
    let Some(url) = part.pointer("/image_url/url").and_then(Value::as_str) else {
        return part.clone();
    };
    let source = match url
        .strip_prefix("data:")
        .and_then(|data| data.split_once(";base64,"))
    {
        Some((media_type, data)) => json!({
            "type": "base64", "media_type": media_type, "data": data,
        }),
        None => json!({"type": "url", "url": url}),
    };
    json!({"type": "image", "source": source})
}

pub(in crate::providers::adapter) fn anthropic_messages(request: &ModelRequest) -> Vec<Value> {
    request
        .messages
        .iter()
        .map(|message| {
            let role = if message.role == "assistant" {
                "assistant"
            } else {
                "user"
            };
            json!({"role": role, "content": anthropic_content(message)})
        })
        .collect()
}

pub(in crate::providers::adapter) fn anthropic_tools(request: &ModelRequest) -> Option<Vec<Value>> {
    request.tools.as_ref().map(|tools| {
        tools
            .iter()
            .map(|tool| json!({
                "name": tool.function.name,
                "description": tool.function.description,
                "input_schema": tool.function.parameters.clone().unwrap_or_else(|| json!({"type": "object"})),
            }))
            .collect()
    })
}

pub(in crate::providers::adapter) fn model_response_from_anthropic(
    route_id: &str,
    body: Value,
    elapsed_ms: f64,
) -> ModelResponse {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in body
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => text.push_str(
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            Some("tool_use") => {
                // A tool call without its id, name or input fails the answer
                // with its cause: sent on as a call named "tool" with id
                // "tool" and empty input, the caller ran or answered a call
                // the model never made.
                let field = |name: &str| block.get(name);
                let (Some(id), Some(name), Some(input)) = (
                    field("id").and_then(Value::as_str),
                    field("name").and_then(Value::as_str),
                    field("input"),
                ) else {
                    return attempted_failure(
                        route_id,
                        Refusal::new(
                            ProviderRefusal::ProviderFailure,
                            format!(
                                "the provider answered a tool_use block without its id, name or input: {block}"
                            ),
                        ),
                    );
                };
                tool_calls.push(ToolCall {
                    id: id.to_string(),
                    call_type: "function".into(),
                    function: crate::types::ToolCallFunction {
                        name: name.to_string(),
                        arguments: input.to_string(),
                    },
                })
            }
            _ => {}
        }
    }
    let (input_tokens, output_tokens, usage_stated) = stated_usage(
        body.pointer("/usage/input_tokens"),
        body.pointer("/usage/output_tokens"),
    );
    ModelResponse {
        content: text,
        model: route_id.to_string(),
        input_tokens,
        output_tokens,
        usage_stated,
        latency_ms: elapsed_ms,
        cost: 0.0,
        success: true,
        attempts: 1,
        error: None,
        tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
        limits: Vec::new(),
        failure_kind: None,
    }
}
