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

// `POST /v1/admin/subscription-pool/grant` stood here until 2026-09-20: the
// console handed Brama a grant it already had, usually one a coding harness
// on some machine was signed into. It is gone. A provider issues one OAuth
// pair per sign-in and revokes it when a second holder refreshes, so every
// adopted grant was a session somebody else lost — twice on this fleet, the
// second time the operator's own, mid-session. A gateway signs itself in:
// `subscription sign-in` through Weles on its own host, or
// `subscription sign-in-manual`, which runs the provider's OAuth flow here
// and mints a pair that belongs to this gateway.

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

/// `POST /v1/admin/subscription-pool/reinstate`: a member this deployment
/// uses after all.
///
/// The mirror of `disown`, and the reason it exists: a retirement was
/// permanent, so a member given back could never be used again by any
/// command, and a pool could report itself empty while the accounts sat in
/// the vault. Reinstating is not a credential: the member returns to the
/// rotation and still needs a grant of
/// this gateway's own, so the answer names the sign-in that obtains one.
pub(in crate::core::server) async fn reinstate_admin_grant(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<DisownRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let reason = request
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .unwrap_or("reinstated by the console; this deployment uses that account");
    crate::subscription_dispatch::pool::reinstate_member(&request.subscription_id, reason)
        .await
        .map(|verdict| {
            Json(json!({
                "ok": true,
                "subscription_id": verdict["subscription_id"],
                "provider": verdict["provider"],
                "detail": verdict["detail"],
            }))
        })
        .map_err(|detail| {
            let status = if detail.contains("holds no member") {
                StatusCode::NOT_FOUND
            } else if detail.contains("is not retired") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            api_error(status, &detail)
        })
}

/// What the console names when it gives a member back.
#[derive(serde::Deserialize)]
pub(in crate::core::server) struct DisownRequest {
    pub subscription_id: String,
    pub reason: Option<String>,
}
