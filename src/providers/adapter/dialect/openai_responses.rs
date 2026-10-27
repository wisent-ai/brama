//! What input the Responses backend accepts.

pub(in crate::providers::adapter) mod event_stream;

use serde_json::{json, Value};

use super::named_tool_choice;
use crate::types::ModelRequest;

/// The tool choice the Codex client sends the Responses backend whenever it
/// sends tools.
const CODEX_TOOL_CHOICE: &str = "auto"; // https://github.com/openai/codex/blob/main/codex-rs/core/src/client.rs ResponsesApiRequest { tool_choice: "auto", .. }

/// The caller's tool choice in the Responses shape: a named function becomes
/// the Responses function choice, and any other choice is carried as stated.
fn responses_tool_choice(choice: &Value) -> Value {
    match named_tool_choice(choice) {
        Some(name) => json!({"type": "function", "name": name}),
        None => choice.clone(),
    }
}

/// Every tool call and tool result was checked at the door
/// (wire::validate_history_tool_calls): a call has an id, a function name and
/// JSON arguments, and a result names an earlier call. Each is carried as
/// stated, with no stand-in for a missing part.
fn responses_input(request: &ModelRequest) -> Vec<Value> {
    let mut input = Vec::with_capacity(request.messages.len());
    for message in &request.messages {
        if message.role == "tool" {
            input.push(json!({
                "type": "function_call_output",
                "call_id": message.tool_call_id,
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
                input.push(json!({
                    "type": "function_call",
                    "call_id": call.get("id"),
                    "name": call.pointer("/function/name"),
                    "arguments": call.pointer("/function/arguments"),
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
                .map(|tool| {
                    json!({
                        "type": "function",
                        "name": tool.function.name,
                        "description": tool.function.description,
                        // A tool that declares no parameters states none.
                        "parameters": tool.function.parameters.clone(),
                    })
                })
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
        // A caller that states no tool choice gets the one the Codex client
        // itself sends this backend with every tool list.
        body["tool_choice"] = match request.tool_choice.as_ref() {
            Some(choice) => responses_tool_choice(choice),
            None => json!(CODEX_TOOL_CHOICE),
        };
    }
    // The answer schema is held by decoding, as the Codex client sends it
    // for an output schema:
    // https://platform.openai.com/docs/api-reference/responses/create#responses-create-text
    if let Some(schema) = &request.response_schema {
        body["text"] = json!({
            "format": {
                "type": "json_schema",
                "name": schema.name,
                "schema": schema.schema,
                "strict": true,
            }
        });
    }
    body
}
