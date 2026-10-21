//! Reading the buffered event stream the Responses backend answers with.

use serde_json::Value;

use crate::providers::adapter::call::outcome::refusal::attempted_failure;
use crate::providers::adapter::stated_usage;
use crate::types::{ModelResponse, ProviderRefusal, Refusal, ToolCall};

/// The class of a failure the backend reported inside a 200 stream, from the
/// error object's `code` (the Responses API's documented error codes, read the
/// way OpenAI's own Codex client reads them), since no HTTP status is left to
/// read once the stream has started.
fn stream_failure_class(error: Option<&Value>) -> ProviderRefusal {
    match error
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
    {
        Some("rate_limit_exceeded" | "slow_down") => ProviderRefusal::RateLimited,
        Some("insufficient_quota") => ProviderRefusal::QuotaExhausted,
        Some("context_length_exceeded") => ProviderRefusal::ContextLengthExceeded,
        Some("server_error") => ProviderRefusal::DependencyUnavailable,
        _ => ProviderRefusal::ProviderFailure,
    }
}

/// Parse a buffered `text/event-stream` body from the OpenAI Responses API
/// into the shared response shape. Deltas accumulate content, but a
/// `response.completed` event's `output` array is the final source of truth.
pub(in crate::providers::adapter) fn model_response_from_responses_stream(
    route_id: &str,
    body: &str,
    elapsed_ms: f64,
) -> ModelResponse {
    let mut content = String::new();
    // The terminal `response.completed` or `response.incomplete` event's
    // response; a stream that ends without one was cut short, and its text
    // so far passed on as a whole answer.
    let mut completed = None;
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
        // An event that is not JSON fails the answer naming it, so the text or
        // call it carries never vanishes from an answer read as complete.
        let event = match serde_json::from_str::<Value>(data) {
            Ok(event) => event,
            Err(error) => {
                let cause = format!("the provider stream sent a non-JSON event ({error}): {data}");
                return provider_failure(route_id, cause);
            }
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
                        Some("function_call") => match function_call(item) {
                            Ok(call) => done_tool_calls.push(call),
                            Err(cause) => return provider_failure(route_id, cause),
                        },
                        _ => {}
                    }
                }
            }
            Some("response.completed") | Some("response.incomplete") => {
                completed = event.get("response").cloned();
            }
            // A failure without a message is reported with the event as sent:
            // a fixed sentence in its place dropped what the provider said.
            Some("response.failed") => {
                let error = event.pointer("/response/error");
                let message = match error
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                {
                    Some(message) => message.to_string(),
                    None => format!("codex response failed without a message: {event}"),
                };
                failure = Some((message, stream_failure_class(error)));
            }
            Some("error") => {
                let message = match event.get("message").and_then(Value::as_str) {
                    Some(message) => message.to_string(),
                    None => format!("codex stream error without a message: {event}"),
                };
                failure = Some((message, stream_failure_class(Some(&event))));
            }
            _ => {}
        }
    }
    if let Some((message, class)) = failure {
        return attempted_failure(
            route_id,
            Refusal::new(class, format!("{}: {message}", class.contract_kind())),
        );
    }
    let Some(completed) = completed else {
        return attempted_failure(
            route_id,
            Refusal::new(
                ProviderRefusal::ProviderFailure,
                "the provider stream ended without response.completed or response.incomplete"
                    .to_string(),
            ),
        );
    };
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
                Some("function_call") => match function_call(item) {
                    Ok(call) => tool_calls.push(call),
                    Err(cause) => return provider_failure(route_id, cause),
                },
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
    let stated = match stated_usage(
        usage.and_then(|usage| usage.get("input_tokens")),
        usage.and_then(|usage| usage.get("output_tokens")),
    ) {
        Ok(stated) => stated,
        Err(cause) => return provider_failure(route_id, cause),
    };
    ModelResponse::answered(
        route_id,
        content,
        stated,
        elapsed_ms,
        (!tool_calls.is_empty()).then_some(tool_calls),
    )
}

/// A Responses `function_call` item as a tool call, or what it lacked. Read
/// with id and name "tool" and arguments "{}", the caller ran or answered a
/// call the model never made.
fn function_call(item: &Value) -> Result<ToolCall, String> {
    let text = |value: Option<&Value>, field: &str| {
        value
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                format!("the provider answered a function_call without its {field}: {item}")
            })
    };
    Ok(ToolCall {
        id: text(item.get("call_id").or_else(|| item.get("id")), "call_id")?,
        call_type: "function".into(),
        function: crate::types::ToolCallFunction {
            name: text(item.get("name"), "name")?,
            arguments: text(item.get("arguments"), "arguments")?,
        },
    })
}

/// The attempt failed because the provider's answer could not be read.
fn provider_failure(route_id: &str, cause: String) -> ModelResponse {
    attempted_failure(
        route_id,
        Refusal::new(ProviderRefusal::ProviderFailure, cause),
    )
}
