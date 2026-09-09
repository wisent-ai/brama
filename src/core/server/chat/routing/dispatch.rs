//! The one attempt the routing decision names, inside the request deadline.
//!
//! Which of the dispatch entry points is called follows entirely from the plan
//! the decision produced, so this file holds no policy: it is the same ladder
//! twice, once for a committed stream and once for a buffered answer.

use axum::http::HeaderMap;

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::subscription_dispatch::{
    dispatch_any_subscription, dispatch_any_subscription_stream,
    dispatch_any_vision_capable_subscription, dispatch_any_vision_capable_subscription_stream,
    dispatch_best_subscription, dispatch_best_subscription_for_agent,
    dispatch_best_subscription_stream, dispatch_best_subscription_stream_for_agent,
    dispatch_direct, dispatch_direct_stream, dispatch_subscription,
    dispatch_subscription_for_agent, dispatch_subscription_stream,
    dispatch_subscription_stream_for_agent, dispatch_task_subscription,
    dispatch_task_subscription_stream,
};
use crate::types::{ModelRequest, ModelResponse};

use super::super::request::request_deadline;
use super::DispatchedCall;

/// Everything the decision established about one call, so the ladder below
/// reads a plan rather than re-deriving it.
pub(super) struct DispatchPlan<'a> {
    pub(super) client_identity: &'a ModelClientIdentity,
    pub(super) headers: &'a HeaderMap,
    pub(super) raw_body: &'a axum::body::Bytes,
    pub(super) request: &'a ModelRequest,
    pub(super) task_subscription: Option<&'a str>,
    pub(super) any_subscription: bool,
    pub(super) any_vision_capable_subscription: bool,
    pub(super) best_subscription: bool,
    pub(super) preferred_route: Option<&'a str>,
    pub(super) account_agent: Option<&'a str>,
    pub(super) caller_scoped_request: bool,
    pub(super) stream: bool,
}

pub(super) async fn dispatch(plan: DispatchPlan<'_>, selected_model: &str) -> DispatchedCall {
    let DispatchPlan {
        client_identity,
        headers,
        raw_body,
        request,
        task_subscription,
        any_subscription,
        any_vision_capable_subscription,
        best_subscription,
        preferred_route,
        account_agent,
        caller_scoped_request,
        stream,
    } = plan;
    // The deadline bounds route selection, rotation, and the provider's
    // response headers. For a stream it stops there by design: once events
    // flow, "how long may this take" is the provider's idle interval, not a
    // budget that would cut a generation mid-sentence.
    tokio::time::timeout(request_deadline(), async {
        if stream {
            let opened = if let Some(task) = task_subscription {
                dispatch_task_subscription_stream(headers, request, raw_body, task).await
            } else if any_vision_capable_subscription {
                dispatch_any_vision_capable_subscription_stream(headers, request, raw_body).await
            } else if any_subscription {
                dispatch_any_subscription_stream(headers, request, raw_body).await
            } else if best_subscription {
                if let Some(agent_id) = client_identity.agent_id() {
                    dispatch_best_subscription_stream_for_agent(agent_id, request, preferred_route)
                        .await
                } else {
                    dispatch_best_subscription_stream(headers, request, raw_body, preferred_route)
                        .await
                }
            } else if let Some(account_agent) = account_agent {
                dispatch_subscription_stream_for_agent(account_agent, request).await
            } else if caller_scoped_request {
                dispatch_subscription_stream(headers, request, raw_body).await
            } else {
                dispatch_direct_stream(request).await
            };
            match opened {
                Ok(routed) => DispatchedCall::Committed(routed),
                Err(failure) => DispatchedCall::Buffered(failure),
            }
        } else if let Some(task) = task_subscription {
            DispatchedCall::Buffered(
                dispatch_task_subscription(headers, request, raw_body, task).await,
            )
        } else if any_vision_capable_subscription {
            DispatchedCall::Buffered(
                dispatch_any_vision_capable_subscription(headers, request, raw_body).await,
            )
        } else if any_subscription {
            DispatchedCall::Buffered(dispatch_any_subscription(headers, request, raw_body).await)
        } else if best_subscription {
            DispatchedCall::Buffered(if let Some(agent_id) = client_identity.agent_id() {
                dispatch_best_subscription_for_agent(agent_id, request, preferred_route).await
            } else {
                dispatch_best_subscription(headers, request, raw_body, preferred_route).await
            })
        } else if let Some(account_agent) = account_agent {
            DispatchedCall::Buffered(dispatch_subscription_for_agent(account_agent, request).await)
        } else if caller_scoped_request {
            DispatchedCall::Buffered(dispatch_subscription(headers, request, raw_body).await)
        } else {
            DispatchedCall::Buffered(dispatch_direct(request).await)
        }
    })
    .await
    .unwrap_or_else(|_| {
        DispatchedCall::Buffered(ModelResponse::failure(
            selected_model,
            "whole request deadline exceeded".into(),
        ))
    })
}
