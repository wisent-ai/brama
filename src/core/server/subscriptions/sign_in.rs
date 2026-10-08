//! Signing one pooled account back in, from the three places the question is
//! asked: an account holder for its own subscription, the console for a named
//! agent's, and the console for a pooled account with no agent in the path.
//!
//! All three narrow to the same execution, which re-reads the account rather
//! than trusting the path: a retired or inactive subscription is refused, and a
//! Weles login item that disagrees with the stored one is a `409` rather than a
//! sign-in against the wrong identity.

pub(in crate::core::server) mod manual;

use axum::body::Body;
use axum::extract::{Extension, Path};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::core::server::administration::require_brama_desktop;
use crate::core::server::admission::identity::{valid_agent_id, ModelClientIdentity};
use crate::core::server::refusal::{api_error, ApiError};

use super::account::account_agent_id;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct SignInSubscriptionRequest {
    subscription_id: Option<String>,
    reason: Option<String>,
    login_item: Option<String>,
}

/// Who logs the account in, stated by every pool sign-in: Weles drives its
/// sign-in row, or the operator logs in in their own browser and pastes the
/// code back to `/v1/admin/subscription-pool/sign-in/:sign_in_id`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::core::server) enum SignInMethod {
    Weles,
    Hand,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct PoolSignInRequest {
    subscription_id: Option<String>,
    reason: Option<String>,
    login_item: Option<String>,
    by: SignInMethod,
}

pub(in crate::core::server) async fn sign_in_account_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Path(subscription_id): Path<String>,
    Json(request): Json<SignInSubscriptionRequest>,
) -> Result<Response, ApiError> {
    let agent_id = account_agent_id(&client_identity)?;
    sign_in_selected_subscription(Some(&agent_id), &subscription_id, request).await
}

pub(in crate::core::server) async fn sign_in_admin_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Path((agent_id, subscription_id)): Path<(String, String)>,
    Json(request): Json<SignInSubscriptionRequest>,
) -> Result<Response, ApiError> {
    require_brama_desktop(&client_identity)?;
    if !valid_agent_id(&agent_id) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid agent id"));
    }
    sign_in_selected_subscription(Some(&agent_id), &subscription_id, request).await
}

/// `POST /v1/admin/subscription-pool/sign-in`: one operation, two methods.
/// `"by": "weles"` streams the Weles sign-in like the other two routes;
/// `"by": "hand"` answers the page to open and the `sign_in_id` the paste
/// completes. A body without `by` is refused by serde: nothing is assumed.
pub(in crate::core::server) async fn sign_in_admin_pool_subscription(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<PoolSignInRequest>,
) -> Result<Response, ApiError> {
    require_brama_desktop(&client_identity)?;
    let subscription_id = request
        .subscription_id
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "a subscription id is required"))?;
    match request.by {
        SignInMethod::Weles => {
            let weles = SignInSubscriptionRequest {
                subscription_id: None,
                reason: request.reason,
                login_item: request.login_item,
            };
            sign_in_selected_subscription(None, &subscription_id, weles).await
        }
        SignInMethod::Hand => {
            if let Some(login_item) = request.login_item {
                return Err(api_error(
                    StatusCode::BAD_REQUEST,
                    &format!("login_item {login_item} names a Weles sign-in row, and by hand signs in through the operator's own browser; drop it or sign in by weles"),
                ));
            }
            let reason = request.reason.ok_or_else(|| {
                api_error(
                    StatusCode::BAD_REQUEST,
                    "--reason must say why this sign-in is being run",
                )
            })?;
            manual::begin_hand_sign_in(&subscription_id, &reason)
                .await
                .map(IntoResponse::into_response)
        }
    }
}

/// A refusal of the request itself is an ordinary error status. An admitted
/// sign-in answers `application/x-ndjson`, one JSON object per line, while it
/// runs: every event Weles reports (`admitted`, `started`, `stage`,
/// `operator_request`), each with the `sentence` the CLI prints for it, and
/// last either `verdict` (the recorded sign-in verdict) or `refused` (the
/// status and error the sign-in stopped on before Weles ran it). A browser
/// sign-in has no clock that ends it; this is how Desktop says what it waits
/// for instead of "Signing in" until it ends.
async fn sign_in_selected_subscription(
    agent_id: Option<&str>,
    subscription_id: &str,
    request: SignInSubscriptionRequest,
) -> Result<Response, ApiError> {
    let reason = request
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_REQUEST,
                "--reason must say why this sign-in is being run",
            )
        })?;
    if request
        .subscription_id
        .as_deref()
        .is_some_and(|id| id != subscription_id)
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "subscription id does not match the requested account",
        ));
    }
    let entries = match agent_id {
        Some(agent_id) => crate::gateway::broker::discover_subscriptions(agent_id).await,
        None => crate::gateway::broker::list_all_subscriptions().await,
    }
    .map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.id == subscription_id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "subscription not found"))?;
    if entry.status != "active" || crate::journal::is_retired(&entry.id) {
        return Err(api_error(
            StatusCode::CONFLICT,
            "retired subscriptions cannot be signed in",
        ));
    }
    if let (Some(held), Some(asked)) = (entry.login_item.as_deref(), request.login_item.as_deref())
    {
        if held != asked.trim() {
            return Err(api_error(
                StatusCode::CONFLICT,
                &format!("subscription `{subscription_id}` uses Weles login item `{held}`, not `{asked}`"),
            ));
        }
    }
    let (events, received) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let progress_events = events.clone();
    let options = crate::subscription_dispatch::sign_in::SignInOptions {
        provider: entry.provider,
        subscription_id: Some(entry.id),
        login_item: request.login_item.or(entry.login_item),
        reason: reason.to_string(),
        progress: Some(std::sync::Arc::new(move |event: &Value| {
            let mut event = event.clone();
            event["sentence"] = json!(crate::subscription_dispatch::sign_in::progress_sentence(
                &event
            ));
            // A Desktop that went away does not stop the sign-in.
            let _ = progress_events.send(event);
        })),
    };
    tokio::spawn(async move {
        let last = match crate::subscription_dispatch::sign_in::sign_in_provider(options).await {
            Ok(verdict) => json!({"event": "verdict", "verdict": verdict}),
            // A missing declaration is this deployment's own configuration,
            // not a bad gateway upstream: Desktop reads the status apart from
            // the sentence, so the two must not both say `502`.
            Err(error) => {
                let status = if error.blocked().is_some() {
                    StatusCode::CONFLICT
                } else {
                    StatusCode::BAD_GATEWAY
                };
                json!({"event": "refused", "status": status.as_u16(), "error": error.to_string()})
            }
        };
        let _ = events.send(last);
    });
    let lines = futures_util::stream::unfold(received, |mut received| async move {
        let event = received.recv().await?;
        Some((
            Ok::<_, std::convert::Infallible>(format!("{event}\n")),
            received,
        ))
    });
    Ok((
        [(header::CONTENT_TYPE, SIGN_IN_PROGRESS_CONTENT_TYPE)],
        Body::from_stream(lines),
    )
        .into_response())
}

const SIGN_IN_PROGRESS_CONTENT_TYPE: &str = "application/x-ndjson";
