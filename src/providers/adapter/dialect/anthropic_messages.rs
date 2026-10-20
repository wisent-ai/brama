//! What a conversation, its tools, a tool choice and an answer look like in the
//! Anthropic Messages dialect.

use serde_json::{json, Value};

use super::named_tool_choice;
use crate::providers::adapter::call::outcome::refusal::attempted_failure;
use crate::providers::adapter::stated_usage;
use crate::types::{
    Message, ModelRequest, ModelResponse, ProviderRefusal, Refusal, ResponseSchema, ToolCall,
};

fn anthropic_tool_choice(choice: &Value) -> Option<Value> {
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

/// One history message in the Anthropic shape, or the reason it cannot be
/// carried: a tool call whose arguments do not read as JSON names the call.
fn anthropic_content(message: &Message) -> Result<Value, String> {
    if message.role == "tool" {
        return Ok(json!([{
            "type": "tool_result",
            "tool_use_id": message.tool_call_id,
            "content": message.content_text(),
        }]));
    }
    if let Some(calls) = &message.tool_calls {
        let mut blocks = Vec::new();
        let text = message.content_text();
        if !text.is_empty() {
            blocks.push(json!({"type": "text", "text": text}));
        }
        // Every history tool call was checked at the door for an id, a
        // function name and JSON arguments (wire::validate_history_tool_calls);
        // one that still does not read is refused with its cause, never sent
        // with a null input.
        for call in calls {
            let arguments = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("history tool call {call} carries no text arguments"))?;
            let input = serde_json::from_str::<Value>(arguments).map_err(|error| {
                format!("history tool call {call} carries arguments that are not JSON: {error}")
            })?;
            blocks.push(json!({
                "type": "tool_use",
                "id": call.get("id"),
                "name": call.pointer("/function/name"),
                "input": input,
            }));
        }
        return Ok(Value::Array(blocks));
    }
    Ok(match &message.content {
        Value::Array(parts) => Value::Array(parts.iter().map(anthropic_block).collect()),
        content => content.clone(),
    })
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

/// The conversation in the Anthropic shape, or the first message that cannot
/// be carried and why.
pub(in crate::providers::adapter) fn anthropic_messages(
    request: &ModelRequest,
) -> Result<Vec<Value>, String> {
    request
        .messages
        .iter()
        .map(|message| -> Result<Value, String> {
            let role = if message.role == "assistant" {
                "assistant"
            } else {
                "user"
            };
            Ok(json!({"role": role, "content": anthropic_content(message)?}))
        })
        .collect()
}

fn anthropic_tools(request: &ModelRequest) -> Option<Vec<Value>> {
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

/// The answer schema an Anthropic request holds through a tool: the caller's
/// declared schema when it declared no tools of its own. The Messages wire
/// holds an answer to a schema with a tool the model must call, whose input
/// schema is that schema
/// (https://docs.anthropic.com/en/docs/build-with-claude/tool-use#json-mode).
pub(in crate::providers::adapter) fn answer_tool(request: &ModelRequest) -> Option<&ResponseSchema> {
    request
        .response_schema
        .as_ref()
        .filter(|_| request.tools.is_none())
}

/// The `tools` and `tool_choice` an Anthropic request carries: the answer
/// tool and a choice that makes the model call it, or the caller's own.
pub(in crate::providers::adapter) fn anthropic_tooling(
    request: &ModelRequest,
) -> (Option<Value>, Option<Value>) {
    if let Some(schema) = answer_tool(request) {
        return (
            Some(json!([{
                "name": schema.name,
                "description": "Give the answer as this object.",
                "input_schema": schema.schema,
            }])),
            Some(json!({"type": "tool", "name": schema.name})),
        );
    }
    (
        anthropic_tools(request).map(Value::from),
        request.tool_choice.as_ref().and_then(anthropic_tool_choice),
    )
}

/// One Messages answer. `answer_tool` is the schema the request held through
/// a forced tool ([`answer_tool`]): that call's input is the answer text, as
/// a wire that decodes to a schema would have written it.
pub(in crate::providers::adapter) fn model_response_from_anthropic(
    route_id: &str,
    body: Value,
    elapsed_ms: f64,
    answer_tool: Option<&ResponseSchema>,
) -> ModelResponse {
    // An answer without a content list is a failure naming what came back:
    // read as no blocks, it passed on as a successful answer that said
    // nothing.
    let Some(blocks) = body.get("content").and_then(Value::as_array) else {
        return attempted_failure(
            route_id,
            Refusal::new(
                ProviderRefusal::ProviderFailure,
                format!("the provider answered without a content list: {body}"),
            ),
        );
    };
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => match block.get("text").and_then(Value::as_str) {
                Some(part) => text.push_str(part),
                None => {
                    return attempted_failure(
                        route_id,
                        Refusal::new(
                            ProviderRefusal::ProviderFailure,
                            format!("the provider answered a text block without its text: {block}"),
                        ),
                    )
                }
            },
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
    if let Some(schema) = answer_tool {
        if let Some(at) = tool_calls
            .iter()
            .position(|call| call.function.name == schema.name)
        {
            text = tool_calls.remove(at).function.arguments;
        }
    }
    let usage = match stated_usage(
        body.pointer("/usage/input_tokens"),
        body.pointer("/usage/output_tokens"),
    ) {
        Ok(usage) => usage,
        Err(cause) => {
            return attempted_failure(
                route_id,
                Refusal::new(ProviderRefusal::ProviderFailure, cause),
            )
        }
    };
    ModelResponse::answered(
        route_id,
        text,
        usage,
        elapsed_ms,
        (!tool_calls.is_empty()).then_some(tool_calls),
    )
}
