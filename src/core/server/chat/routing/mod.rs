//! The routing decision, made once and shared by every wire format.
//!
//! Selector detection, the client's model allowlist, alias resolution, whether
//! the call is caller-scoped -- and therefore billed to the caller's own
//! subscription -- all happen here; [`dispatch`] then performs the one attempt
//! that decision names. `Err` is a refusal that happened before any provider
//! was contacted; a provider failure comes back as [`DispatchedCall::Buffered`]
//! carrying it, because that is the same shape the buffered path has always
//! reported.

pub(super) mod dispatch;

use std::time::Instant;

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use tracing::{info, warn};

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::aliases::BEST_ALIAS;
use crate::core::server::refusal::{api_error, api_error_with_details};
use crate::core::server::subscriptions::account::account_agent_for_route;
use crate::subscription_dispatch::{
    is_subscription_model, provider_requires_caller_identity, RoutedStream,
};
use crate::types::{ModelRequest, ModelResponse};

use super::request::{
    is_any_subscription_selector, is_any_vision_capable_subscription_selector,
    task_subscription_selector,
};

use dispatch::DispatchPlan;

/// Whether this call was answered in one piece or committed as a stream.
pub(super) enum DispatchedCall {
    Buffered(ModelResponse),
    Committed(RoutedStream),
}

/// What the routing decision produced besides the call itself, so every
/// format shapes its answer from the same facts.
pub(super) struct RoutedCallMeta {
    pub(super) routing_mode: &'static str,
    alias_source: Option<String>,
    pub(super) request_id: String,
    pub(super) selected_model: String,
    pub(super) started: Instant,
}

impl RoutedCallMeta {
    /// The model name the caller should be told about: the alias it asked for
    /// when one resolved, and the canonical route it actually reached
    /// otherwise. An alias exists so a caller does not have to know which
    /// route is behind it today.
    pub(super) fn reported_model(&self, requested_model: &str, resolved: &str) -> String {
        if self.alias_source.is_some() {
            requested_model.to_string()
        } else {
            resolved.to_string()
        }
    }
}

/// Resolve one model call and execute it, buffered or streamed.
pub(super) async fn route_model_call(
    client_identity: &ModelClientIdentity,
    aliases: &ModelAliases,
    headers: &HeaderMap,
    raw_body: &axum::body::Bytes,
    requested_model: &str,
    mut request: ModelRequest,
    stream: bool,
) -> Result<(DispatchedCall, RoutedCallMeta), Response> {
    let any_subscription = is_any_subscription_selector(requested_model);
    let any_vision_capable_subscription =
        is_any_vision_capable_subscription_selector(requested_model);
    let account_agent = account_agent_for_route(client_identity, requested_model).await;
    if !client_identity.authorizes_model(requested_model) && account_agent.is_none() {
        return Err(api_error(StatusCode::FORBIDDEN, "forbidden").into_response());
    }
    let task_subscription = task_subscription_selector(requested_model);
    let alias_source = aliases.chat_route(requested_model);
    // An alias the gateway knows but cannot serve is a configuration fault on
    // this host, not a malformed request. Answering "unknown model" made it
    // one: the requested name is not a provider/model route, so the caller was
    // told its model name was wrong, and the catalogue had already dropped the
    // alias rather than list it as unavailable. Both answers hid the same fact.
    if alias_source.is_none()
        && requested_model != BEST_ALIAS
        && aliases.is_declared(requested_model)
    {
        let diagnosis = aliases.diagnose(requested_model);
        if !diagnosis.serving() {
            warn!(
                event = "alias_unserviceable_request",
                client_id = %client_identity.client_id,
                alias = requested_model,
                state = diagnosis.state,
                "refusing a declared alias that has no serviceable route"
            );
            return Err(api_error_with_details(
                StatusCode::SERVICE_UNAVAILABLE,
                "alias_unserviceable",
                &diagnosis.reason.clone().unwrap_or_else(|| {
                    format!("alias `{requested_model}` has no serviceable route")
                }),
                json!({
                    "alias": diagnosis.alias,
                    "state": diagnosis.state,
                    "route": diagnosis.route,
                }),
            )
            .into_response());
        }
    }
    // `best` is a selector, not a route. The alias resolves to one configured
    // provider route and that route is the caller's first choice, but the name
    // means "the best subscription model this caller can be served from", so
    // the rest of the caller's ranked routes stand behind it. Dispatching the
    // configured route alone is what answered `503` naming codex, with
    // `attempts: 0`, in a second when the same agent's kimi subscription was
    // serving: one refused redemption and the request was over.
    let best_subscription = requested_model == BEST_ALIAS
        || alias_source
            .as_deref()
            .is_some_and(|route| route == BEST_ALIAS);
    let selected_model = alias_source
        .as_deref()
        .unwrap_or(requested_model)
        .to_string();
    // A route that delegates to `best` again names no provider, so it is a
    // preference nothing can be preferred to.
    let preferred_route = Some(selected_model.as_str())
        .filter(|route| is_subscription_model(route))
        .map(str::to_owned);
    let canonical_model = is_subscription_model(&selected_model);
    if !(any_subscription
        || any_vision_capable_subscription
        || best_subscription
        || task_subscription.is_some()
        || canonical_model)
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "model must be a canonical provider/model route or a supported selector",
        )
        .into_response());
    }
    // Subscription dispatch is for credentials that belong to the caller, and
    // the four tests below say when that is the case: an explicit subscription
    // selector, a named billing target, or a provider whose usage must bill to
    // the caller's own subscription -- `claude-code`, `codex`, `kimi`.
    //
    // Presenting agent auth used to be a fifth. It protected nothing those four
    // do not already cover, and it inverted the access rule: the same client,
    // with the same bearer and the same alias, was served from the deployment's
    // own provider when it stayed anonymous and refused with "no active
    // credential for agent" the moment it proved who it was. The catalogue said
    // the opposite in the same breath, marking those models available to agent
    // callers. Proving more identity cannot grant less access. The dispatch
    // branches below must therefore use this predicate too; a bearer-bound
    // `agent_id` remains audit identity, not an entitlement by itself.
    let caller_scoped_request = account_agent.is_some()
        || any_subscription
        || any_vision_capable_subscription
        || best_subscription
        || task_subscription.is_some()
        || request.billing_target.is_some()
        || provider_requires_caller_identity(&selected_model);
    let routing_mode = if task_subscription.is_some() {
        "task"
    } else if any_vision_capable_subscription {
        "any-vision-capable"
    } else if any_subscription {
        "any"
    } else if best_subscription {
        "best"
    } else if caller_scoped_request {
        "subscription"
    } else if alias_source.is_some() {
        "alias"
    } else {
        "direct"
    };
    let request_id = format!("chatcmpl-{}", uuid_v4());
    info!(
        event = "routing_decision",
        request_id = %request_id,
        client_id = %client_identity.client_id,
        agent_id = client_identity
            .agent_id()
            .or(account_agent.as_deref())
            .unwrap_or("none"),
        routing_mode,
        requested_model,
        selected_model = %selected_model,
        streamed = stream,
        "routing request accepted"
    );
    let started = Instant::now();
    request.model = selected_model.clone();
    let meta = RoutedCallMeta {
        routing_mode,
        alias_source,
        request_id,
        selected_model,
        started,
    };
    let dispatched = dispatch::dispatch(
        DispatchPlan {
            client_identity,
            headers,
            raw_body,
            request: &request,
            task_subscription: task_subscription.as_deref(),
            any_subscription,
            any_vision_capable_subscription,
            best_subscription,
            preferred_route: preferred_route.as_deref(),
            account_agent: account_agent.as_deref(),
            caller_scoped_request,
            stream,
        },
        &meta.selected_model,
    )
    .await;
    Ok((dispatched, meta))
}

fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{t:032x}")
}
