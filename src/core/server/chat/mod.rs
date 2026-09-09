//! One model request, executed. `POST /v1/chat/completions` lives here; the
//! two other formats a caller may speak live in [`dialects`] and reach the very
//! same [`routing`] decision, so nothing about identity, billing or bounded
//! attempts differs between them.
//!
//! [`request`] holds the wire shapes and the limits, [`outcome`] the counters
//! and the log line, and the streamed answer is encoded in
//! [`crate::core::server::streaming`].

pub(in crate::core::server) mod dialects;
mod outcome;
pub(in crate::core::server) mod request;
mod routing;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::refusal::api_error;
use crate::core::server::streaming::{chunks::ChatChunkStream, sse_response};
use crate::types::{Message, ModelRequest};

use outcome::{failure_response, log_stream_commit, tally_and_log_buffered};
use request::{
    max_output_tokens, max_temperature, ChatCompletionRequest, ChatCompletionResponse, Choice,
    ChoiceMessage, Usage,
};
use routing::{route_model_call, DispatchedCall};

pub(in crate::core::server) async fn chat_completions(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let req: ChatCompletionRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(error) => {
            return api_error(StatusCode::BAD_REQUEST, &format!("invalid JSON: {error}"))
                .into_response();
        }
    };
    if req.messages.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "messages must not be empty").into_response();
    }
    if req.max_tokens == u32::default() || req.max_tokens > max_output_tokens() {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!("max_tokens must be between one and {}", max_output_tokens()),
        )
        .into_response();
    }
    if !req.temperature.is_finite()
        || req.temperature < f64::default()
        || req.temperature > max_temperature()
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "temperature must be finite and between zero and {}",
                max_temperature()
            ),
        )
        .into_response();
    }
    let messages: Vec<Message> = req
        .messages
        .into_iter()
        .map(|m| Message {
            role: m.role,
            content: m
                .content
                .unwrap_or_else(|| serde_json::Value::String(String::new())),
            tool_call_id: m.tool_call_id,
            name: m.name,
            tool_calls: m.tool_calls,
        })
        .collect();
    let requested_model = req.model.as_deref().unwrap_or("").trim().to_string();
    if requested_model.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "missing field `model`").into_response();
    }
    let (system, non_system_messages) = split_system_message(messages);
    let request = ModelRequest {
        messages: non_system_messages,
        model: String::new(),
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        system,
        tools: req.tools,
        tool_choice: req.tool_choice,
        billing_target: req.billing_target,
    };
    let (dispatched, meta) = match route_model_call(
        &client_identity,
        &aliases,
        &headers,
        &body,
        &requested_model,
        request,
        req.stream,
    )
    .await
    {
        Ok(routed) => routed,
        Err(response) => return response,
    };
    match dispatched {
        DispatchedCall::Buffered(mut resp) => {
            tally_and_log_buffered(&mut resp, &client_identity, &meta, &requested_model);
            if !resp.success {
                return failure_response(&mut resp);
            }
            // Alias telemetry stays attached to the stable logical selector.
            crate::core::perf::record(&requested_model, resp.latency_ms, resp.output_tokens);
            let has_tool_calls = resp.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty());
            let finish_reason = if has_tool_calls { "tool_calls" } else { "stop" };
            let response_model = meta.reported_model(&requested_model, &resp.model);
            let body = serde_json::to_value(ChatCompletionResponse {
                id: meta.request_id.clone(),
                object: "chat.completion".into(),
                model: response_model,
                choices: vec![Choice {
                    index: 0,
                    message: ChoiceMessage {
                        role: "assistant".into(),
                        content: resp.content,
                        tool_calls: resp.tool_calls,
                    },
                    finish_reason: finish_reason.into(),
                }],
                usage: Usage {
                    prompt_tokens: resp.input_tokens,
                    completion_tokens: resp.output_tokens,
                    total_tokens: resp.input_tokens + resp.output_tokens,
                },
            })
            .unwrap_or_default();
            (StatusCode::OK, Json(body)).into_response()
        }
        DispatchedCall::Committed(routed) => {
            log_stream_commit(&routed, &client_identity, &meta, &requested_model);
            let model = meta.reported_model(&requested_model, &routed.model);
            sse_response(ChatChunkStream::new(
                routed.events,
                meta.request_id.clone(),
                model,
                requested_model,
                meta.started,
            ))
        }
    }
}

/// Split the OpenAI system-role message out of a conversation.
///
/// The stateless provider adapters take the system prompt as its own field, so
/// the first system-role message becomes that field and the rest of the
/// conversation travels unchanged.
fn split_system_message(messages: Vec<Message>) -> (Option<String>, Vec<Message>) {
    let mut system: Option<String> = None;
    let mut rest: Vec<Message> = Vec::with_capacity(messages.len());
    for message in messages {
        if message.role == "system" && system.is_none() {
            system = message.content.as_str().map(|value| value.to_string());
            continue;
        }
        rest.push(message);
    }
    (system, rest)
}
