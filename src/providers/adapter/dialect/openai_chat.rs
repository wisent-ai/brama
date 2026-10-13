//! What a conversation and an answer look like in the OpenAI Chat dialect.

use serde_json::{json, Value};

use crate::providers::adapter::call::outcome::refusal::attempted_failure;
use crate::providers::adapter::stated_usage;
use crate::types::{
    ModelRequest, ModelResponse, ProviderRefusal, Refusal, ResponseSchema, ToolCall,
};

/// The chat wire's `response_format` for an answer schema. Strict, so a
/// provider that decodes under the schema — OpenAI, and vLLM through its
/// guided decoding — cannot write a key or a label outside it.
pub(in crate::providers::adapter) fn response_format(schema: &ResponseSchema) -> Value {
    json!({
        "type": "json_schema",
        "json_schema": { "name": schema.name, "strict": true, "schema": schema.schema },
    })
}

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
    // A tool call the model made but this gateway cannot read fails the
    // answer with its cause: dropped, the caller got an answer without a call
    // the model asked for.
    let tool_calls = match choice.get("tool_calls").and_then(Value::as_array) {
        None => None,
        Some(calls) => match calls
            .iter()
            .map(|call| serde_json::from_value::<ToolCall>(call.clone()))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(calls) => Some(calls).filter(|calls| !calls.is_empty()),
            Err(error) => {
                return attempted_failure(
                    route_id,
                    Refusal::new(
                        ProviderRefusal::ProviderFailure,
                        format!(
                            "the provider answered a tool call this gateway cannot read: {error}"
                        ),
                    ),
                )
            }
        },
    };
    let (input_tokens, output_tokens, usage_stated) = stated_usage(
        body.pointer("/usage/prompt_tokens"),
        body.pointer("/usage/completion_tokens"),
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
        tool_calls,
        limits: Vec::new(),
        failure_kind: None,
    }
}
