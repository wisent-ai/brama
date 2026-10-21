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
    // Built from the message's own fields, so nothing can fail to encode and
    // nothing is dropped; the tool name is not part of the chat wire.
    messages.extend(request.messages.iter().map(|message| {
        let mut object = serde_json::Map::new();
        object.insert("role".into(), json!(message.role));
        object.insert("content".into(), message.content.clone());
        if let Some(id) = &message.tool_call_id {
            object.insert("tool_call_id".into(), json!(id));
        }
        if let Some(calls) = &message.tool_calls {
            object.insert("tool_calls".into(), Value::Array(calls.clone()));
        }
        Value::Object(object)
    }));
    messages
}

pub(in crate::providers::adapter) fn model_response_from_openai(
    route_id: &str,
    body: Value,
    elapsed_ms: f64,
) -> ModelResponse {
    // An answer without a first choice's message is a failure naming what
    // came back, never a successful answer that says nothing.
    let Some(choice) = body.pointer("/choices/0/message").cloned() else {
        return attempted_failure(
            route_id,
            Refusal::new(
                ProviderRefusal::ProviderFailure,
                format!("the provider answered without choices[0].message: {body}"),
            ),
        );
    };
    // Content is text or absent (null beside tool calls); anything else is
    // a body this gateway cannot read, and says so.
    let content = match choice.get("content") {
        Some(Value::String(text)) => text.clone(),
        None | Some(Value::Null) => String::new(),
        Some(other) => {
            return attempted_failure(
                route_id,
                Refusal::new(
                    ProviderRefusal::ProviderFailure,
                    format!("the provider answered message content that is not text: {other}"),
                ),
            )
        }
    };
    // A tool call the model made but this gateway cannot read fails the
    // answer with its cause, so the caller never gets an answer without a
    // call the model asked for.
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
    // Neither text nor a call is an answer that said nothing: a failure
    // naming the body, not a successful empty answer.
    if content.is_empty() && tool_calls.is_none() {
        return attempted_failure(
            route_id,
            Refusal::new(
                ProviderRefusal::ProviderFailure,
                format!("the provider answered with neither content nor tool calls: {body}"),
            ),
        );
    }
    match stated_usage(
        body.pointer("/usage/prompt_tokens"),
        body.pointer("/usage/completion_tokens"),
    ) {
        Ok(usage) => ModelResponse::answered(route_id, content, usage, elapsed_ms, tool_calls),
        Err(cause) => attempted_failure(
            route_id,
            Refusal::new(ProviderRefusal::ProviderFailure, cause),
        ),
    }
}
