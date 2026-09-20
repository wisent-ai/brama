//! The manual sign-in as Brama Desktop asks for it: two steps, because the
//! browser is the operator's and the gateway is not on the operator's machine.
//!
//! `begin` draws the PKCE verifier and hands back the page to open; the
//! verifier stays here, keyed by a sign-in id, for as long as a Weles login is
//! allowed to take. `complete` takes the code the operator pasted, exchanges
//! it, stores the grant and proves it with one completion - the same `complete` the
//! CLI ends in. A verifier that never leaves the gateway is the whole point:
//! the code alone, seen on a screen or in a paste, buys nothing.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::core::server::administration::require_brama_desktop;
use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::refusal::{api_error, ApiError};
use crate::subscription_dispatch::sign_in::manual::{self, AuthorizationRequest};

/// How long an opened page stays redeemable: the time a Weles-driven login
/// is allowed, so an operator who is slow at a second factor is not refused.
const PENDING_TTL: Duration = Duration::from_secs(900);

/// How many sign-ins may be open at once. One console, one operator, one
/// account at a time; the bound exists so a client that begins and never
/// completes cannot grow this map.
const PENDING_LIMIT: usize = 8;

struct Pending {
    request: AuthorizationRequest,
    reason: String,
    started: Instant,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Default::default);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct BeginRequest {
    subscription_id: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct CompleteRequest {
    code: String,
}

/// A grant the console already holds - read from a harness on the machine
/// the console runs on - handed over as the document Brama's refresh path
/// reads for the provider.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct GrantRequest {
    subscription_id: String,
    reason: String,
    /// The harness the console read it from, when it read it from one.
    #[serde(default)]
    harness: Option<String>,
    #[serde(default)]
    account: Option<String>,
    /// The provider the grant belongs to. Required when `subscription_id`
    /// names a pool member that does not exist yet: the grant then creates
    /// it. On 2026-09-17 the pool routed every Claude call through one
    /// rate-limited account while the operator's machine held two more with
    /// quota, and the only way to add them was a sign-in window - the
    /// operator's words were "mamy limit, tylko Ty patrzysz na złe konta".
    #[serde(default)]
    provider: Option<String>,
    document: Zeroizing<String>,
}

/// The active, unretired pooled account a request names, or the refusal
/// that says why it cannot be signed in.
async fn active_account(
    subscription_id: &str,
    reason: &str,
) -> Result<crate::gateway::broker::SubscriptionEntry, ApiError> {
    if reason.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "--reason must say why this sign-in is being run",
        ));
    }
    if subscription_id.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "a subscription id is required",
        ));
    }
    let entries = crate::gateway::broker::list_all_subscriptions()
        .await
        .map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?;
    let entry = entries
        .into_iter()
        .find(|entry| entry.id == subscription_id.trim())
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "subscription not found"))?;
    if entry.status != "active" || crate::journal::is_retired(&entry.id) {
        return Err(api_error(
            StatusCode::CONFLICT,
            "retired subscriptions cannot be signed in",
        ));
    }
    Ok(entry)
}

/// `POST /v1/admin/subscription-pool/sign-in-manual`: the page to open.
pub(in crate::core::server) async fn begin_admin_manual_sign_in(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<BeginRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let reason = request.reason.trim();
    let entry = active_account(&request.subscription_id, reason).await?;
    let authorization = manual::begin(&entry.provider, &entry.id)
        .map_err(|detail| api_error(StatusCode::CONFLICT, &detail))?;
    let sign_in_id = authorization.state.clone();
    let url = authorization.url.clone();
    let mut pending = PENDING.lock().expect("pending manual sign-ins");
    pending.retain(|_, open| open.started.elapsed() < PENDING_TTL);
    if pending.len() >= PENDING_LIMIT {
        return Err(api_error(
            StatusCode::TOO_MANY_REQUESTS,
            "too many manual sign-ins are open; complete or let one expire first",
        ));
    }
    pending.insert(
        sign_in_id.clone(),
        Pending {
            request: authorization,
            reason: reason.to_owned(),
            started: Instant::now(),
        },
    );
    Ok(Json(json!({
        "sign_in_id": sign_in_id,
        "provider": entry.provider,
        "subscription_id": entry.id,
        "url": url,
        "expires_in_secs": PENDING_TTL.as_secs(),
    })))
}

/// `POST /v1/admin/subscription-pool/sign-in-manual/:sign_in_id`: the paste.
pub(in crate::core::server) async fn complete_admin_manual_sign_in(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Path(sign_in_id): Path<String>,
    Json(request): Json<CompleteRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let pending = {
        let mut pending = PENDING.lock().expect("pending manual sign-ins");
        pending.retain(|_, open| open.started.elapsed() < PENDING_TTL);
        pending.remove(&sign_in_id)
    };
    let Some(pending) = pending else {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "no manual sign-in with that id is open; begin one again and paste what the new page shows",
        ));
    };
    let verdict = manual::complete(pending.request, &request.code, &pending.reason)
        .await
        .map_err(|detail| api_error(StatusCode::CONFLICT, &detail))?;
    Ok(Json(
        serde_json::to_value(verdict).expect("verdict serializes"),
    ))
}

/// `POST /v1/admin/subscription-pool/grant`: a grant the console holds,
/// stored and proved like one the provider just issued. An id the pool does
/// not hold yet becomes a new member of the named provider; storing the
/// grant creates and tags its vault item, so discovery sees it at once.
pub(in crate::core::server) async fn adopt_admin_grant(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<GrantRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let (provider, subscription_id) = match active_account(
        &request.subscription_id,
        &request.reason,
    )
    .await
    {
        Ok(entry) => (entry.provider, entry.id),
        Err(refusal) if refusal.0 == StatusCode::NOT_FOUND => {
            let provider = request
                .provider
                .as_deref()
                .map(str::trim)
                .filter(|provider| !provider.is_empty())
                .ok_or_else(|| {
                    api_error(
                        StatusCode::NOT_FOUND,
                        "subscription not found; name its provider to add it to the pool with this grant",
                    )
                })?;
            if !crate::gateway::broker::supports_oauth_refresh(provider) {
                return Err(api_error(
                    StatusCode::BAD_REQUEST,
                    &format!("`{provider}` is not a provider whose grants Brama keeps"),
                ));
            }
            let subscription_id = request.subscription_id.trim().to_owned();
            if !crate::core::server::administration::valid_alias(&subscription_id) {
                return Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "invalid subscription id",
                ));
            }
            (provider.to_owned(), subscription_id)
        }
        Err(refusal) => return Err(refusal),
    };
    let origin = match request.harness.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => {
            manual::Origin::Harness(manual::Harness::parse(name).ok_or_else(|| {
                api_error(
                    StatusCode::BAD_REQUEST,
                    &format!("`{name}` is not a harness Brama reads grants from"),
                )
            })?)
        }
        _ => manual::Origin::Console,
    };
    let verdict = manual::adopt(
        &provider,
        &subscription_id,
        request.document,
        request.account,
        origin,
        request.reason.trim(),
    )
    .await
    .map_err(|detail| api_error(StatusCode::CONFLICT, &detail))?;
    Ok(Json(
        serde_json::to_value(verdict).expect("verdict serializes"),
    ))
}

/// `POST /v1/admin/subscription-pool/disown`: the console takes back a grant
/// it adopted.
///
/// The agent-facing `retire` accepts only the agent that banked a member, and
/// a grant the console adopted was banked by nobody: on 2026-09-20 the three
/// claude-code accounts imported from a workstation could not be given back
/// by any caller, while the provider kept revoking that workstation's own
/// session because two machines held one pair. What the console created, the
/// console can retire.
pub(in crate::core::server) async fn disown_admin_grant(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<DisownRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let subscription_id = request.subscription_id.trim();
    if subscription_id.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "subscription_id is required",
        ));
    }
    let provider = crate::gateway::broker::discover_subscriptions("brama-desktop")
        .await
        .map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?
        .into_iter()
        .find(|entry| entry.id == subscription_id)
        .map(|entry| entry.provider);
    let Some(provider) = provider else {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "the pool holds no member with that id",
        ));
    };
    crate::journal::retire(subscription_id);
    crate::subscription_dispatch::usage::record_credential_disabled(
        subscription_id,
        &provider,
        request
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .unwrap_or("disowned by the console that adopted it"),
    );
    crate::gateway::broker::donated_remove(subscription_id)
        .map_err(|detail| api_error(StatusCode::CONFLICT, &detail))?;
    crate::gateway::broker::remove_donated_credential(&provider, subscription_id)
        .map_err(|detail| api_error(StatusCode::CONFLICT, &detail))?;
    Ok(Json(json!({
        "ok": true,
        "subscription_id": subscription_id,
        "provider": provider,
        "detail": "the member is retired and its stored credential removed; the machine its grant came from keeps its own session",
    })))
}

/// What the console names when it gives a member back.
#[derive(serde::Deserialize)]
pub(in crate::core::server) struct DisownRequest {
    pub subscription_id: String,
    pub reason: Option<String>,
}
