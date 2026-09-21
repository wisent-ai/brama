//! `POST /v1/decisions`: the endpoint that answers typed questions instead of
//! generating text.
//!
//! It accepts exactly two model names, `decision-model` and
//! `best-decision-model`, for the same reason the embeddings endpoint accepts
//! exactly one: the name is the caller's promise that it will get a typed
//! answer, and the route behind it is the deployment's business. The first
//! resolves to a route this gateway pays for; the second delegates to `best`,
//! so the caller's own signed identity selects the subscription that pays,
//! exactly as it does for generation.
//!
//! What happens between the alias and the answer is in [`crate::core::decisions`].

use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::core::decisions::{
    decide_for_agent, decide_on_direct_route, outcome_from_chat, prompt, DecisionFailure,
    DecisionOutcome, DecisionRequest,
};
use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::aliases::{BEST_ALIAS, DECISION_ALIAS, DECISION_ALIASES};
use crate::core::server::refusal::envelope::typed_dispatch_error;
use crate::core::server::refusal::{api_error, api_error_with_details, ApiError};
use crate::core::server::telemetry::record_decision_request;
use crate::subscription_dispatch::dispatch_best_subscription;

/// The refusal a provider's answer earns when it is outside the schema the
/// caller declared. It is its own code because it is its own repair: nobody
/// fixes it by waiting, and the route that produced it is the thing to change.
const CONTRACT_VIOLATED: &str = "decision_contract_violated";

pub(in crate::core::server) async fn decisions(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    let parsed: Value = serde_json::from_slice(&body)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, &format!("invalid JSON: {error}")))?;
    let request = DecisionRequest::parse(&parsed)
        .map_err(|message| api_error(StatusCode::BAD_REQUEST, &message))?;
    if !DECISION_ALIASES.contains(&request.model.as_str()) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "model must be `decision-model` or `best-decision-model`",
        ));
    }
    if !client_identity.authorizes_model(&request.model) {
        return Err(api_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let Some(route) = aliases.decision_route(&request.model) else {
        let diagnosis = aliases.diagnose(&request.model);
        warn!(
            event = "decision_alias_unserviceable",
            client_id = %client_identity.client_id,
            alias = %request.model,
            state = diagnosis.state,
            "refusing a decision alias that has no serviceable route"
        );
        return Err(api_error_with_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "alias_unserviceable",
            &diagnosis
                .reason
                .clone()
                .unwrap_or_else(|| format!("alias `{}` has no serviceable route", request.model)),
            json!({
                "alias": diagnosis.alias,
                "state": diagnosis.state,
                "route": diagnosis.route,
            }),
        ));
    };
    info!(
        event = "decision_accepted",
        client_id = %client_identity.client_id,
        alias = %request.model,
        route = %route,
        questions = request.questions.len(),
        "decision request accepted"
    );
    let outcome = if route == BEST_ALIAS {
        delegated(&client_identity, &headers, &body, &request).await
    } else {
        decide_on_direct_route(&route, &request).await
    };
    match outcome {
        Ok(outcome) => {
            record_decision_request(outcome.input_tokens, outcome.output_tokens, false);
            Ok(Json(answered(&request.model, &outcome)))
        }
        Err(failure) => {
            record_decision_request(u32::default(), u32::default(), true);
            Err(refused(&request.model, &route, failure))
        }
    }
}

/// `best-decision-model`: the caller's own subscription answers, selected by
/// the identity it proved — a bearer-bound agent id, or the HMAC signature
/// over this very body when the caller signs instead.
async fn delegated(
    client_identity: &ModelClientIdentity,
    headers: &HeaderMap,
    body: &axum::body::Bytes,
    request: &DecisionRequest,
) -> Result<DecisionOutcome, DecisionFailure> {
    if let Some(agent_id) = client_identity.agent_id() {
        return decide_for_agent(BEST_ALIAS, agent_id, request).await;
    }
    let call = prompt::chat_request(request, BEST_ALIAS);
    let response = dispatch_best_subscription(headers, &call, body, None).await;
    outcome_from_chat(BEST_ALIAS, request, response)
}

/// The answer document: the alias the caller named, the route and engine that
/// answered it, and one typed answer per question.
fn answered(alias: &str, outcome: &DecisionOutcome) -> Value {
    json!({
        "model": alias,
        "route": outcome.route,
        "engine": outcome.engine,
        "answers": outcome.answers,
        "usage": {
            "input_tokens": outcome.input_tokens,
            "output_tokens": outcome.output_tokens,
            "latency_ms": (outcome.latency_ms * 100.0).round() / 100.0,
        },
    })
}

fn refused(alias: &str, route: &str, failure: DecisionFailure) -> ApiError {
    match failure {
        DecisionFailure::Provider(message) => {
            warn!(
                event = "decision_provider_failed",
                alias, route, error = %message,
                "the route behind a decision alias refused"
            );
            typed_dispatch_error(&message)
        }
        DecisionFailure::Contract(message) => {
            warn!(
                event = "decision_contract_violated",
                alias, route, error = %message,
                "the model answered outside the schema the caller declared"
            );
            api_error_with_details(
                StatusCode::BAD_GATEWAY,
                CONTRACT_VIOLATED,
                &message,
                json!({
                    "alias": alias,
                    "route": route,
                    "engine_hint": if alias == DECISION_ALIAS {
                        "point this alias at a stronger decision route"
                    } else {
                        "the subscription model behind `best` answered outside the declared schema"
                    },
                }),
            )
        }
    }
}
