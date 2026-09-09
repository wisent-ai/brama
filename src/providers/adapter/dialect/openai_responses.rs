//! What input the Responses backend accepts.

pub(in crate::providers::adapter) mod event_stream;

use serde_json::{json, Value};

use super::named_tool_choice;
use crate::types::ModelRequest;

fn responses_tool_choice(choice: &Value) -> Value {
    named_tool_choice(choice)
        .map(|name| json!({"type": "function", "name": name}))
        .unwrap_or_else(|| choice.clone())
}

fn responses_input(request: &ModelRequest) -> Vec<Value> {
    let mut input = Vec::with_capacity(request.messages.len());
    for message in &request.messages {
        if message.role == "tool" {
            input.push(json!({
                "type": "function_call_output",
                "call_id": message.tool_call_id.as_deref().unwrap_or("tool"),
                "output": message.content_text(),
            }));
            continue;
        }
        if let Some(calls) = &message.tool_calls {
            let text = message.content_text();
            if !text.is_empty() {
                input.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": text}],
                }));
            }
            for call in calls {
                let Some(function) = call.get("function") else {
                    continue;
                };
                input.push(json!({
                    "type": "function_call",
                    "call_id": call.get("id").and_then(Value::as_str).unwrap_or("tool"),
                    "name": function.get("name").and_then(Value::as_str).unwrap_or("tool"),
                    "arguments": function
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}"),
                }));
            }
            continue;
        }
        let role = if message.role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        let content_type = if role == "assistant" {
            "output_text"
        } else {
            "input_text"
        };
        input.push(json!({
            "type": "message",
            "role": role,
            "content": [{"type": content_type, "text": message.content_text()}],
        }));
    }
    input
}

fn responses_tools(request: &ModelRequest) -> Option<Vec<Value>> {
    request
        .tools
        .as_ref()
        .filter(|tools| !tools.is_empty())
        .map(|tools| {
            tools
                .iter()
                .map(|tool| json!({
                    "type": "function",
                    "name": tool.function.name,
                    "description": tool.function.description,
                    "parameters": tool.function.parameters.clone().unwrap_or_else(|| json!({"type": "object"})),
                }))
                .collect()
        })
}

pub(super) fn responses_payload(request: &ModelRequest, model_id: &str) -> Value {
    let mut body = json!({
        "model": model_id,
        "input": responses_input(request),
        "store": false,
        "stream": true,
    });
    if let Some(system) = request.system.as_deref().filter(|value| !value.is_empty()) {
        body["instructions"] = json!(system);
    }
    if let Some(tools) = responses_tools(request) {
        body["tools"] = json!(tools);
        body["tool_choice"] = request
            .tool_choice
            .as_ref()
            .map(responses_tool_choice)
            .unwrap_or_else(|| json!("auto"));
    }
    body
}
