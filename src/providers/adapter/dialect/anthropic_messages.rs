//! What a conversation, its tools, a tool choice and an answer look like in the
//! Anthropic Messages dialect.

use serde_json::{json, Value};

use super::named_tool_choice;
use crate::types::{Message, ModelRequest, ModelResponse, ToolCall};

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
        for call in calls {
            let Some(function) = call.get("function") else {
                continue;
            };
            let input = function
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|value| serde_json::from_str::<Value>(value).ok())
                .unwrap_or_else(|| json!({}));
            blocks.push(json!({
                "type": "tool_use",
                "id": call.get("id").and_then(Value::as_str).unwrap_or("tool"),
                "name": function.get("name").and_then(Value::as_str).unwrap_or("tool"),
                "input": input,
            }));
        }
        return Value::Array(blocks);
    }
    message.content.clone()
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
            Some("tool_use") => tool_calls.push(ToolCall {
                id: block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string(),
                call_type: "function".into(),
                function: crate::types::ToolCallFunction {
                    name: block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string(),
                    arguments: block
                        .get("input")
                        .cloned()
                        .unwrap_or_else(|| json!({}))
                        .to_string(),
                },
            }),
            _ => {}
        }
    }
    ModelResponse {
        content: text,
        model: route_id.to_string(),
        input_tokens: body
            .pointer("/usage/input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        output_tokens: body
            .pointer("/usage/output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        latency_ms: elapsed_ms,
        cost: 0.0,
        success: true,
        attempts: u32::from(true),
        error: None,
        tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
        limits: Vec::new(),
    }
}
