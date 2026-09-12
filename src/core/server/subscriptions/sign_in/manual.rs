//! The manual sign-in as Brama Desktop asks for it: two steps, because the
//! browser is the operator's and the gateway is not on the operator's machine.
//!
//! `begin` draws the PKCE verifier and hands back the page to open; the
//! verifier stays here, keyed by a sign-in id, for as long as a Weles login is
//! allowed to take. `complete` takes the code the operator pasted, exchanges
//! it, stores the grant and proves it with a refresh - the same `finish` the
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

/// `POST /v1/admin/subscription-pool/sign-in-manual`: the page to open.
pub(in crate::core::server) async fn begin_admin_manual_sign_in(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<BeginRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let reason = request.reason.trim();
    if reason.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "--reason must say why this sign-in is being run",
        ));
    }
    let subscription_id = request.subscription_id.trim();
    if subscription_id.is_empty() {
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
        .find(|entry| entry.id == subscription_id)
        .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "subscription not found"))?;
    if entry.status != "active" || crate::journal::is_retired(&entry.id) {
        return Err(api_error(
            StatusCode::CONFLICT,
            "retired subscriptions cannot be signed in",
        ));
    }
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
    let verdict = manual::finish(pending.request, &request.code, &pending.reason)
        .await
        .map_err(|detail| api_error(StatusCode::CONFLICT, &detail))?;
    Ok(Json(
        serde_json::to_value(verdict).expect("verdict serializes"),
    ))
}
