//! What a conversation and an answer look like in the OpenAI Chat dialect.

use serde_json::{json, Value};

use crate::types::{ModelRequest, ModelResponse, ToolCall};

pub(in crate::providers::adapter) fn openai_messages(request: &ModelRequest) -> Vec<Value> {
    let mut messages =
        Vec::with_capacity(request.messages.len() + usize::from(request.system.is_some()));
    if let Some(system) = request.system.as_deref().filter(|value| !value.is_empty()) {
        messages.push(json!({"role": "system", "content": system}));
    }
    messages.extend(request.messages.iter().map(|message| {
        let mut value = serde_json::to_value(message).unwrap_or_else(|_| {
            json!({
                "role": message.role,
                "content": message.content,
            })
        });
        if let Some(object) = value.as_object_mut() {
            object.remove("name");
        }
        value
    }));
    messages
}

pub(in crate::providers::adapter) fn model_response_from_openai(
    route_id: &str,
    body: Value,
    elapsed_ms: f64,
) -> ModelResponse {
    let choice = body
        .pointer("/choices/0/message")
        .cloned()
        .unwrap_or(Value::Null);
    let content = choice
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let tool_calls = choice
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| serde_json::from_value::<ToolCall>(call.clone()).ok())
                .collect::<Vec<_>>()
        })
        .filter(|calls| !calls.is_empty());
    ModelResponse {
        content,
        model: route_id.to_string(),
        input_tokens: body
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        output_tokens: body
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        latency_ms: elapsed_ms,
        cost: 0.0,
        success: true,
        attempts: u32::from(true),
        error: None,
        tool_calls,
        limits: Vec::new(),
    }
}
