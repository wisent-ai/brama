//! The subscription pool: read it, write it, and read what its plans have
//! left. Which accounts an answer carries follows from the identity the caller
//! proved, so there is one scoping decision in this gateway rather than one per
//! audience.

pub(in crate::core::server) mod account;
mod membership;
pub(in crate::core::server) mod probe;
pub(in crate::core::server) mod sign_in;

use axum::extract::Extension;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use serde_json::Value;

use crate::core::server::administration::require_brama_desktop;
use crate::core::server::admission::identity::{valid_agent_id, ModelClientIdentity};
use crate::core::server::admission::{authorize_caller, has_caller_auth_headers};
use crate::core::server::refusal::{api_error, ApiError};
use crate::subscription_dispatch::{plan_usage, pool};

use account::account_agent_id;
use membership::{create_subscription, retire_managed_subscription, DonateSubscriptionRequest};

/// One membership change to the pool: `bank` a credential onto an account, or
/// `retire` one out of the pool.
///
/// `deny_unknown_fields` is what makes a mistyped field a refusal rather than
/// a silently dropped intention. A donation whose `subscription_id` was
/// quietly ignored overwrites the deterministic primary account instead of the
/// one the caller named, and the one copy of a working credential is what it
/// overwrites.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubscriptionPoolWrite {
    action: Option<String>,
    agent_id: Option<String>,
    provider: Option<String>,
    label: Option<String>,
    api_key: Option<String>,
    login_item: Option<String>,
    subscription_id: Option<String>,
}

/// Which pool one caller may be answered about, from what that caller proved.
///
/// Three audiences reach this capability and none of them is taken at its
/// word: the signed agent proves an agent, the account holder proves a Wisent
/// user, and the console proves this installation. Ownership is read off the
/// proof rather than off a path segment or a body field -- both of which the
/// caller chooses -- so the three answers are one answer narrowed three ways
/// instead of three implementations that can disagree.
///
/// Order matters. A caller presenting the HMAC trio is held to it even when
/// its bearer would also pass for something wider: a signature over this exact
/// body is the strongest statement available, and ignoring a broken one to
/// serve the request on a weaker credential is how a mis-signed mutation gets
/// through.
async fn subscription_pool_scope(
    identity: &ModelClientIdentity,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<pool::PoolScope, ApiError> {
    if has_caller_auth_headers(headers) {
        let caller = authorize_caller(identity, headers, body, None).await?;
        if !valid_agent_id(&caller) {
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid agent id"));
        }
        return Ok(pool::PoolScope::Agent(caller));
    }
    if identity.human_context().is_some() {
        return Ok(pool::PoolScope::Agent(account_agent_id(identity)?));
    }
    require_brama_desktop(identity)?;
    Ok(pool::PoolScope::Deployment)
}

/// The subscription pool, answered once, for whoever proved they may read it.
pub(in crate::core::server) async fn read_subscription_pool(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let scope = subscription_pool_scope(&client_identity, &headers, &[]).await?;
    Ok(Json(pool::report(&scope).await))
}

/// Subscription plan usage: what every plan the caller proved it owns has
/// left, read from the usage ledger and each provider's own usage report.
///
/// One sweep and one ledger were asked for four ways -- a signed agent's
/// refresh, an account holder's, an administered agent's, and the console's
/// whole-pool refresh -- and every one of them read this ledger and these
/// reports. This answers it once and narrows the answer by the identity the
/// caller proved, through the same scope the pool resolves: there is one
/// scoping decision in this gateway, not one per family.
///
/// It is a `POST` because answering reads each provider's own usage report and
/// records what they said. That costs no plan quota: no completion is sent, no
/// sign-in is started, and a provider that publishes no free report says so
/// rather than being reported as unused.
///
/// The signed body is empty and stays empty. A caller has nothing to say here
/// -- the answer follows from what it proved, not from what it asked for -- so
/// a request that carries a body is refused instead of being answered from a
/// signature over something this handler then ignores.
pub(in crate::core::server) async fn read_plan_usage(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    let scope = subscription_pool_scope(&client_identity, &headers, &body).await?;
    if !body.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "plan usage takes no request fields; the answer follows from the proven identity",
        ));
    }
    Ok(Json(plan_usage::report(&scope).await))
}

/// Bank a credential into the pool, or retire one out of it.
///
/// One surface for both, because both are the same statement about membership
/// made by the same proven owner against the same declaration. Only the
/// console may name an agent, being the only caller whose proof is not itself
/// an agent; for anybody else naming one would be choosing an owner rather
/// than proving it, which is exactly what the per-audience routes let a caller
/// attempt.
pub(in crate::core::server) async fn write_subscription_pool(
    Extension(client_identity): Extension<ModelClientIdentity>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    let scope = subscription_pool_scope(&client_identity, &headers, &body).await?;
    let request: SubscriptionPoolWrite = serde_json::from_slice(&body).map_err(|error| {
        api_error(
            StatusCode::BAD_REQUEST,
            &format!("invalid subscription request: {error}"),
        )
    })?;
    let named_agent = request.agent_id.as_deref().map(str::trim);
    let agent_id = match (&scope, named_agent) {
        (pool::PoolScope::Agent(proven), None) => proven.clone(),
        (pool::PoolScope::Agent(_), Some(_)) => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "agent_id is derived from the proven identity and must not be sent",
            ))
        }
        (pool::PoolScope::Deployment, Some(named)) if valid_agent_id(named) => named.to_owned(),
        (pool::PoolScope::Deployment, Some(_)) => {
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid agent id"))
        }
        (pool::PoolScope::Deployment, None) => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "agent_id names the agent whose pool is written and is required for a \
                 deployment-scoped write",
            ))
        }
    };
    match request.action.as_deref().map(str::trim) {
        Some("bank") => {
            create_subscription(
                agent_id,
                DonateSubscriptionRequest {
                    provider: request.provider,
                    label: request.label,
                    api_key: request.api_key,
                    login_item: request.login_item,
                    subscription_id: request.subscription_id,
                },
            )
            .await
        }
        Some("retire") => {
            let subscription_id = request
                .subscription_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| api_error(StatusCode::BAD_REQUEST, "subscription_id is required"))?
                .to_owned();
            retire_managed_subscription(agent_id, subscription_id).await
        }
        _ => Err(api_error(
            StatusCode::BAD_REQUEST,
            "action must be \"bank\" or \"retire\"",
        )),
    }
}
