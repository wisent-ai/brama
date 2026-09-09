//! Turning one `ModelRequest` into the body a given wire protocol expects.

pub(in crate::providers::adapter) mod anthropic_messages;
pub(in crate::providers::adapter) mod openai_chat;
pub(in crate::providers::adapter) mod openai_responses;
pub(in crate::providers::adapter) mod tool_schema;

use serde_json::{json, Map, Value};

use super::registry::{ProviderDescriptor, WireProtocol};
use crate::types::ModelRequest;
use anthropic_messages::{anthropic_tool_choice, anthropic_tools};
use openai_chat::openai_messages;
use openai_responses::responses_payload;
use tool_schema::normalized_tools_value;

pub(in crate::providers::adapter) fn named_tool_choice(choice: &Value) -> Option<&str> {
    choice
        .pointer("/function/name")
        .or_else(|| choice.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
}

/// The non-streaming chat payload for one wire protocol.
///
/// This is the exact body [`super::dispatch`] has always sent; both dispatch
/// paths build from it so a streaming request differs from a buffered one only
/// by the flags [`streaming_chat_payload`] adds.
pub(in crate::providers::adapter) fn chat_payload(
    descriptor: &ProviderDescriptor,
    model_id: &str,
    request: &ModelRequest,
) -> Value {
    match descriptor.wire {
        WireProtocol::OpenAiChat => {
            let mut body = Map::new();
            body.insert("model".into(), json!(model_id));
            body.insert("messages".into(), Value::Array(openai_messages(request)));
            body.insert("max_tokens".into(), json!(request.max_tokens));
            // kimi-for-coding pins temperature to 1 and rejects any other value.
            if descriptor.id != "kimi" {
                body.insert("temperature".into(), json!(request.temperature));
            }
            if let Some(tools) = &request.tools {
                body.insert("tools".into(), normalized_tools_value(tools));
            }
            if let Some(choice) = &request.tool_choice {
                body.insert("tool_choice".into(), choice.clone());
            }
            Value::Object(body)
        }
        WireProtocol::AnthropicMessages => {
            let mut body = json!({
                "model": model_id,
                "messages": self::anthropic_messages::anthropic_messages(request),
                "max_tokens": request.max_tokens,
                "temperature": request.temperature,
            });
            if let Some(system) = request.system.as_deref().filter(|value| !value.is_empty()) {
                body["system"] = json!(system);
            }
            if let Some(tools) = anthropic_tools(request) {
                body["tools"] = json!(tools);
            }
            if let Some(choice) = request.tool_choice.as_ref().and_then(anthropic_tool_choice) {
                body["tool_choice"] = choice;
            }
            body
        }
        WireProtocol::OpenAiResponses => responses_payload(request, model_id),
    }
}

/// The same payload asking the provider to stream.
///
/// Anthropic and the Responses backend stream on one flag. The chat wire
/// additionally asks for the terminal usage chunk -- except Kimi, whose
/// coding endpoint's accepted field set is pinned and whose streams therefore
/// carry no usage at all; the ledger records what a Kimi stream measured as
/// no reading rather than an invented one.
pub(in crate::providers::adapter) fn streaming_chat_payload(
    descriptor: &ProviderDescriptor,
    model_id: &str,
    request: &ModelRequest,
) -> Value {
    let mut body = chat_payload(descriptor, model_id, request);
    match descriptor.wire {
        WireProtocol::OpenAiChat => {
            body["stream"] = json!(true);
            if descriptor.id != "kimi" {
                body["stream_options"] = json!({ "include_usage": true });
            }
        }
        WireProtocol::AnthropicMessages => {
            body["stream"] = json!(true);
        }
        // The Responses payload is streamed by construction already; the
        // buffered path parses its buffered event body.
        WireProtocol::OpenAiResponses => {}
    }
    body
}
