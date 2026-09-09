//! Reading the buffered event stream the Responses backend answers with.

use serde_json::Value;

use crate::providers::adapter::call::refusal::attempted_failure;
use crate::types::{ModelResponse, ToolCall};

/// Parse a buffered `text/event-stream` body from the OpenAI Responses API
/// into the shared response shape. Deltas accumulate content, but a
/// `response.completed` event's `output` array is the final source of truth.
pub(in crate::providers::adapter) fn model_response_from_responses_stream(
    route_id: &str,
    body: &str,
    elapsed_ms: f64,
) -> ModelResponse {
    let mut content = String::new();
    let mut completed = Value::Null;
    let mut failure = None;
    let mut done_text = String::new();
    let mut done_tool_calls = Vec::new();
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_text.delta") => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    content.push_str(delta);
                }
            }
            Some("response.output_item.done") => {
                // Codex's completed event ships output: [] — the real message
                // and function_call items only arrive in output_item.done.
                if let Some(item) = event.get("item") {
                    match item.get("type").and_then(Value::as_str) {
                        Some("message") => {
                            for part in item
                                .get("content")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                            {
                                if let Some(value) = part.get("text").and_then(Value::as_str) {
                                    done_text.push_str(value);
                                }
                            }
                        }
                        Some("function_call") => done_tool_calls.push(ToolCall {
                            id: item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(Value::as_str)
                                .unwrap_or("tool")
                                .to_string(),
                            call_type: "function".into(),
                            function: crate::types::ToolCallFunction {
                                name: item
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or("tool")
                                    .to_string(),
                                arguments: item
                                    .get("arguments")
                                    .and_then(Value::as_str)
                                    .unwrap_or("{}")
                                    .to_string(),
                            },
                        }),
                        _ => {}
                    }
                }
            }
            Some("response.completed") => {
                completed = event.get("response").cloned().unwrap_or(Value::Null);
            }
            Some("response.failed") => {
                let message = event
                    .pointer("/response/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("codex response failed");
                failure = Some(message.to_string());
            }
            Some("error") => {
                let message = event
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("codex stream error");
                failure = Some(message.to_string());
            }
            _ => {}
        }
    }
    if let Some(message) = failure {
        return attempted_failure(route_id, format!("provider_failure: {message}"));
    }
    let mut tool_calls = Vec::new();
    if let Some(output) = completed.get("output").and_then(Value::as_array) {
        let mut text = String::new();
        for item in output {
            match item.get("type").and_then(Value::as_str) {
                Some("message") => {
                    for part in item
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        if let Some(value) = part.get("text").and_then(Value::as_str) {
                            text.push_str(value);
                        }
                    }
                }
                Some("function_call") => tool_calls.push(ToolCall {
                    id: item
                        .get("call_id")
                        .or_else(|| item.get("id"))
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string(),
                    call_type: "function".into(),
                    function: crate::types::ToolCallFunction {
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool")
                            .to_string(),
                        arguments: item
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}")
                            .to_string(),
                    },
                }),
                _ => {}
            }
        }
        if !text.is_empty() {
            content = text;
        }
    }
    if content.is_empty() && !done_text.is_empty() {
        content = done_text;
    }
    for call in done_tool_calls {
        if !tool_calls.iter().any(|existing| existing.id == call.id) {
            tool_calls.push(call);
        }
    }
    let usage = completed.get("usage");
    ModelResponse {
        content,
        model: route_id.to_string(),
        input_tokens: usage
            .and_then(|usage| usage.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        output_tokens: usage
            .and_then(|usage| usage.get("output_tokens"))
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
