//! The two other formats a caller may speak, on the same routing decision.
//!
//! Nothing about routing, identity, or billing changes with the wire format:
//! the model must still be a canonical route or a supported selector, a
//! subscription still needs the caller's own signed identity, and the same
//! bounded attempts apply. Only the request and response shapes differ, and
//! they are translated in [`crate::core::wire`].

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::refusal::api_error;
use crate::core::server::streaming::{epoch_seconds, sse_response, stream_accounting};

use super::outcome::{failure_response, log_stream_commit, tally_and_log_buffered};
use super::routing::{route_model_call, DispatchedCall};

/// One Anthropic Messages request, routed exactly as its chat-completions
/// sibling is and answered in the format the caller speaks.
pub(in crate::core::server) async fn anthropic_messages(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let call = match crate::core::wire::anthropic_request(&body) {
        Ok(call) => call,
        Err(message) => return api_error(StatusCode::BAD_REQUEST, &message).into_response(),
    };
    let requested_model = call.model;
    let (dispatched, meta) = match route_model_call(
        &client_identity,
        &aliases,
        &headers,
        &body,
        &requested_model,
        call.request,
        call.stream,
    )
    .await
    {
        Ok(routed) => routed,
        Err(response) => return response,
    };
    let message_id = format!("msg_{}", uuid::Uuid::new_v4().simple());
    match dispatched {
        DispatchedCall::Buffered(mut resp) => {
            tally_and_log_buffered(&mut resp, &client_identity, &meta, &requested_model);
            if !resp.success {
                return failure_response(&mut resp);
            }
            crate::core::perf::record(&requested_model, resp.latency_ms, resp.output_tokens);
            let model = meta.reported_model(&requested_model, &resp.model);
            (
                StatusCode::OK,
                Json(crate::core::wire::anthropic_response(
                    &message_id,
                    &model,
                    &resp,
                )),
            )
                .into_response()
        }
        DispatchedCall::Committed(routed) => {
            log_stream_commit(&routed, &client_identity, &meta, &requested_model);
            let model = meta.reported_model(&requested_model, &routed.model);
            sse_response(crate::core::wire::AnthropicEventStream::new(
                routed.events,
                message_id,
                model,
                stream_accounting(requested_model, meta.started),
            ))
        }
    }
}

/// One OpenAI Responses request, on the same routing decision as every other
/// format this gateway serves.
pub(in crate::core::server) async fn openai_responses(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let call = match crate::core::wire::responses_request(&body) {
        Ok(call) => call,
        Err(message) => return api_error(StatusCode::BAD_REQUEST, &message).into_response(),
    };
    let requested_model = call.model;
    let (dispatched, meta) = match route_model_call(
        &client_identity,
        &aliases,
        &headers,
        &body,
        &requested_model,
        call.request,
        call.stream,
    )
    .await
    {
        Ok(routed) => routed,
        Err(response) => return response,
    };
    let response_id = format!("resp_{}", uuid::Uuid::new_v4().simple());
    match dispatched {
        DispatchedCall::Buffered(mut resp) => {
            tally_and_log_buffered(&mut resp, &client_identity, &meta, &requested_model);
            if !resp.success {
                return failure_response(&mut resp);
            }
            crate::core::perf::record(&requested_model, resp.latency_ms, resp.output_tokens);
            let model = meta.reported_model(&requested_model, &resp.model);
            (
                StatusCode::OK,
                Json(crate::core::wire::responses_response(
                    &response_id,
                    &model,
                    epoch_seconds(),
                    &resp,
                )),
            )
                .into_response()
        }
        DispatchedCall::Committed(routed) => {
            log_stream_commit(&routed, &client_identity, &meta, &requested_model);
            let model = meta.reported_model(&requested_model, &routed.model);
            sse_response(crate::core::wire::ResponsesEventStream::new(
                routed.events,
                response_id,
                model,
                stream_accounting(requested_model, meta.started),
            ))
        }
    }
}
