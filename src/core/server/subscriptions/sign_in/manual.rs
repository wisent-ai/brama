//! The sign-in `by: "hand"` as Brama Desktop asks for it: two steps, because
//! the browser is the operator's and the gateway is not on the operator's machine.
//!
//! `begin` draws the PKCE verifier and hands back the page to open; the
//! verifier stays here, keyed by a sign-in id, until the operator pastes the
//! code or begins again for the same account. `complete` takes the pasted
//! code, exchanges it, stores the grant and proves it with one completion -
//! the same `complete` the
//! CLI ends in. A verifier that never leaves the gateway is the whole point:
//! the code alone, seen on a screen or in a paste, buys nothing.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use axum::extract::{Extension, Path};
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

pub(in crate::core::server) mod member;

use crate::core::server::administration::require_brama_desktop;
use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::refusal::{api_error, ApiError};
use crate::subscription_dispatch::sign_in::manual::{self, AuthorizationRequest};

/// An open sign-in. Each pooled account holds at most one: beginning again
/// for the same account replaces the page it had open, so the map holds no
/// more entries than the pool has accounts and nothing expires on a clock.
struct Pending {
    request: AuthorizationRequest,
    reason: String,
    target: member::Target,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Default::default);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct CompleteRequest {
    code: String,
}

/// `POST /v1/admin/subscription-pool/sign-in` with `"by": "hand"`: the page to
/// open. The caller has admitted the console and refused a Weles login item;
/// `joining` names a member a standalone gateway banks by this sign-in.
pub(in crate::core::server) async fn begin_hand_sign_in(
    subscription_id: &str,
    reason: &str,
    joining: Option<member::Joining>,
    console: &str,
) -> Result<Json<Value>, ApiError> {
    let reason = reason.trim();
    let target = member::target(subscription_id, reason, joining, console).await?;
    let authorization = manual::begin(&target.provider, &target.id)
        .map_err(|detail| api_error(StatusCode::CONFLICT, &detail))?;
    let sign_in_id = authorization.state.clone();
    let url = authorization.url.clone();
    let answer = json!({
        "sign_in_id": sign_in_id,
        "provider": target.provider,
        "subscription_id": target.id,
        "url": url,
    });
    let mut pending = crate::core::sync::held(&PENDING, "pending manual sign-ins");
    pending.retain(|_, open| open.target.id != target.id);
    pending.insert(
        sign_in_id,
        Pending {
            request: authorization,
            reason: reason.to_owned(),
            target,
        },
    );
    Ok(Json(answer))
}

/// `POST /v1/admin/subscription-pool/sign-in/:sign_in_id`: the paste.
pub(in crate::core::server) async fn complete_admin_manual_sign_in(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Path(sign_in_id): Path<String>,
    Json(request): Json<CompleteRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let pending = {
        let mut pending = crate::core::sync::held(&PENDING, "pending manual sign-ins");
        pending.remove(&sign_in_id)
    };
    let Some(pending) = pending else {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "no manual sign-in with that id is open; begin one again and paste what the new page shows",
        ));
    };
    // A member this sign-in banks is in the pool before its grant is stored
    // and proved, and out of it again when the exchange fails; a member it
    // did not bank is never taken out.
    let banked = pending.target.bank().await?;
    let verdict = match manual::complete(pending.request, &request.code, &pending.reason).await {
        Ok(verdict) => verdict,
        Err(detail) => {
            let detail = match banked.map(member::Banked::unbank) {
                None | Some(Ok(())) => detail,
                Some(Err(left)) => {
                    format!("{detail}; the member it banked could not be taken out again: {left}")
                }
            };
            return Err(api_error(StatusCode::CONFLICT, &detail));
        }
    };
    let answered = serde_json::to_value(verdict).map_err(|error| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("the manual sign-in completed but its verdict could not be encoded: {error}"),
        )
    })?;
    Ok(Json(answered))
}

// There is no `POST /v1/admin/subscription-pool/grant`: a console that hands
// Brama a grant it already has, usually one a coding harness on some machine
// is signed into, hands it a session somebody else loses. A provider issues
// one OAuth pair per sign-in and revokes it when a second holder refreshes,
// so every adopted grant costs its first holder its session, the operator's
// own included. A gateway signs itself in:
// `subscription login --by weles` through Weles on its own host, or
// `subscription login --by hand`, which runs the provider's OAuth flow here
// and mints a pair that belongs to this gateway.

/// `POST /v1/admin/subscription-pool/disown`: the console takes back a grant
/// it adopted.
///
/// The agent-facing `retire` accepts only the agent that banked a member; a
/// grant the console adopted was banked by no agent, so the console is the
/// one caller that retires it. The reason is the caller's and is required.
pub(in crate::core::server) async fn disown_admin_grant(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<DisownRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let reason = request.stated_reason("disown")?;
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
    crate::journal::retire(subscription_id)
        .map_err(|detail| api_error(StatusCode::INTERNAL_SERVER_ERROR, &detail))?;
    crate::subscription_dispatch::usage::record_credential_disabled(
        subscription_id,
        &provider,
        reason,
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
/// The mirror of `disown`: the member returns to the rotation. Reinstating
/// is not a credential, so the member still needs a grant of this gateway's
/// own, and the answer names the sign-in that obtains one. The reason is the
/// caller's and is required.
pub(in crate::core::server) async fn reinstate_admin_grant(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<DisownRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let reason = request.stated_reason("reinstate")?;
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
        .map_err(|(refused, detail)| {
            use crate::subscription_dispatch::pool::ReinstateRefused;
            let status = match refused {
                ReinstateRefused::NoMember => StatusCode::NOT_FOUND,
                ReinstateRefused::NotRetired => StatusCode::CONFLICT,
                ReinstateRefused::Unreadable => StatusCode::SERVICE_UNAVAILABLE,
                ReinstateRefused::Invalid => StatusCode::BAD_REQUEST,
            };
            api_error(status, &detail)
        })
}

/// `POST /v1/admin/subscription-pool/forget`: a member only the usage
/// ledger remembers is dropped from it, for a reason.
///
/// The usage ledger keeps every member that ever ran here; one the vault no
/// longer returns is no pool member and is not listed, but its row stays.
/// `disown` refuses it (`the pool holds no member with that id`). This is
/// the verb that takes the row out; a member the vault lists is
/// refused back to `disown`. The reason is the caller's and is required,
/// because the journal keeps it as the operator's words.
pub(in crate::core::server) async fn forget_admin_member(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Json(request): Json<DisownRequest>,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let reason = request.stated_reason("forget")?;
    crate::subscription_dispatch::forget::forget_member(&request.subscription_id, reason)
        .await
        .map(|verdict| {
            Json(json!({
                "ok": true,
                "subscription_id": verdict["subscription_id"],
                "provider": verdict["provider"],
                "detail": verdict["detail"],
            }))
        })
        .map_err(|(refused, detail)| {
            use crate::subscription_dispatch::forget::ForgetRefused;
            let status = match refused {
                ForgetRefused::Invalid => StatusCode::BAD_REQUEST,
                ForgetRefused::NoRecord => StatusCode::NOT_FOUND,
                ForgetRefused::Listed | ForgetRefused::Leased => StatusCode::CONFLICT,
                ForgetRefused::Unreadable => StatusCode::SERVICE_UNAVAILABLE,
            };
            api_error(status, &detail)
        })
}

/// What the console names when it gives a member back, takes it again or
/// forgets it.
#[derive(serde::Deserialize)]
pub(in crate::core::server) struct DisownRequest {
    pub subscription_id: String,
    pub reason: Option<String>,
}

impl DisownRequest {
    /// The reason the caller states for `change`. The journal and the usage
    /// ledger keep it as the operator's words, so a missing or blank one is
    /// refused instead of written in their place.
    fn stated_reason(&self, change: &str) -> Result<&str, ApiError> {
        match self.reason.as_deref().map(str::trim) {
            Some(reason) if !reason.is_empty() => Ok(reason),
            _ => Err(api_error(
                StatusCode::BAD_REQUEST,
                &format!("reason is required: say why you {change} this member"),
            )),
        }
    }
}
