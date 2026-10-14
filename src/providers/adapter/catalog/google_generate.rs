//! The Google generateContent dialect, reachable only through the catalog.

use serde_json::{json, Value};

use super::super::dialect::named_tool_choice;
use crate::providers::adapter::stated_usage;
use crate::types::{Message, ModelRequest, ModelResponse, ToolCall};

fn google_tool_config(choice: &Value) -> Option<Value> {
    if let Some(name) = named_tool_choice(choice) {
        return Some(json!({
            "functionCallingConfig": {
                "mode": "ANY",
                "allowedFunctionNames": [name],
            }
        }));
    }
    match choice.as_str() {
        Some("auto") => Some(json!({"functionCallingConfig": {"mode": "AUTO"}})),
        Some("required") => Some(json!({"functionCallingConfig": {"mode": "ANY"}})),
        Some("none") => Some(json!({"functionCallingConfig": {"mode": "NONE"}})),
        _ => None,
    }
}

/// Every tool call and tool result was checked at the door
/// (wire::validate_history_tool_calls): a call has an id, a function name and
/// JSON arguments, and a result names an earlier call by `tool_call_id`. A
/// `functionResponse` is named after the call it answers, found in `names`;
/// read from the result's own optional `name`, it went out as "tool" for
/// every caller that sends results the chat-completions way, and Gemini
/// matched it to no call.
fn google_parts(message: &Message, names: &std::collections::BTreeMap<&str, &Value>) -> Vec<Value> {
    if message.role == "tool" {
        let name = message.tool_call_id.as_deref().and_then(|id| names.get(id));
        return vec![json!({
            "functionResponse": {
                "name": name,
                "response": {"result": message.content_text()},
            }
        })];
    }
    let mut parts = match &message.content {
        Value::String(text) => vec![json!({"text": text})],
        Value::Array(values) => values
            .iter()
            .filter_map(|part| match part.get("type").and_then(Value::as_str) {
                Some("text") => part.get("text").map(|text| json!({"text": text})),
                Some("image_url") => {
                    let url = part.pointer("/image_url/url").and_then(Value::as_str)?;
                    if let Some(data) = url.strip_prefix("data:") {
                        let (mime, encoded) = data.split_once(";base64,")?;
                        Some(json!({"inlineData": {"mimeType": mime, "data": encoded}}))
                    } else {
                        Some(json!({"fileData": {"fileUri": url}}))
                    }
                }
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    if let Some(calls) = &message.tool_calls {
        parts.extend(calls.iter().map(|call| {
            let arguments = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .and_then(|value| serde_json::from_str::<Value>(value).ok());
            json!({
                "functionCall": {
                    "name": call.pointer("/function/name"),
                    "args": arguments,
                }
            })
        }));
    }
    parts
}

pub(super) fn google_payload(request: &ModelRequest) -> Value {
    // Each history tool call's function name by its id, so a tool result is
    // named after the call it answers.
    let names: std::collections::BTreeMap<&str, &Value> = request
        .messages
        .iter()
        .flat_map(|message| message.tool_calls.iter().flatten())
        .filter_map(|call| Some((call.get("id")?.as_str()?, call.pointer("/function/name")?)))
        .collect();
    let mut body = json!({
        "contents": request.messages.iter().map(|message| {
            json!({
                "role": if message.role == "assistant" { "model" } else { "user" },
                "parts": google_parts(message, &names),
            })
        }).collect::<Vec<_>>(),
        "generationConfig": {},
    });
    if let Some(max_tokens) = request.max_tokens {
        body["generationConfig"]["maxOutputTokens"] = json!(max_tokens);
    }
    if let Some(temperature) = request.temperature {
        body["generationConfig"]["temperature"] = json!(temperature);
    }
    if let Some(system) = request.system.as_deref().filter(|value| !value.is_empty()) {
        body["systemInstruction"] = json!({"parts": [{"text": system}]});
    }
    if let Some(tools) = request.tools.as_ref().filter(|tools| !tools.is_empty()) {
        body["tools"] = json!([{
            "functionDeclarations": tools.iter().map(|tool| json!({
                "name": tool.function.name,
                "description": tool.function.description,
                "parameters": tool.function.parameters.clone().unwrap_or_else(|| json!({"type": "object"})),
            })).collect::<Vec<_>>()
        }]);
    }
    if let Some(config) = request.tool_choice.as_ref().and_then(google_tool_config) {
        body["toolConfig"] = config;
    }
    body
}

pub(super) fn model_response_from_google(
    route_id: &str,
    body: Value,
    elapsed_ms: f64,
) -> ModelResponse {
    // A candidate without content parts (one stopped for safety, recitation
    // or length states only its finishReason) and a functionCall without a
    // name are failures naming what came back: read as no parts and as a
    // call named "tool", they passed on as a successful empty answer and a
    // call the model never made.
    let failed = |detail: String| {
        crate::providers::adapter::call::outcome::refusal::attempted_failure(
            route_id,
            crate::types::Refusal::new(crate::types::ProviderRefusal::ProviderFailure, detail),
        )
    };
    let Some(parts) = body
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
    else {
        return failed(format!(
            "the provider answered without candidates[0].content.parts: {body}"
        ));
    };
    let mut content = String::new();
    let mut tool_calls = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            content.push_str(text);
        }
        if let Some(call) = part.get("functionCall") {
            let Some(name) = call.get("name").and_then(Value::as_str) else {
                return failed(format!(
                    "the provider answered a functionCall without a name: {part}"
                ));
            };
            tool_calls.push(ToolCall {
                id: format!("google-call-{index}"),
                call_type: "function".into(),
                function: crate::types::ToolCallFunction {
                    name: name.to_string(),
                    arguments: call
                        .get("args")
                        .cloned()
                        .unwrap_or_else(|| json!({}))
                        .to_string(),
                },
            });
        }
    }
    let (input_tokens, output_tokens, usage_stated) = stated_usage(
        body.pointer("/usageMetadata/promptTokenCount"),
        body.pointer("/usageMetadata/candidatesTokenCount"),
    );
    ModelResponse {
        content,
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
