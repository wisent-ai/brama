//! The caller's side of Wisent's contribution ledger: the console's view of
//! the ledger this gateway declares, and the gate a caller passes to reach an
//! alias served only to holders of its entitlement.
//!
//! The ledger knows a signed-in Wisent user by their account id, the account
//! their donated subscriptions are kept under, and the account links it on
//! its own Rewards page with no code; a workload caller by the agent it
//! signed as, or its client id. Brama links no identity: a workload's
//! credential is presented by every process that holds it, so nothing it
//! sends can show that the person asked.

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value};
use tracing::info;

use crate::core::server::administration::require_brama_desktop;
use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::refusal::{api_error, api_error_with_details, ApiError};
use crate::gateway::broker::rewards::{check_entitlement, gated, status, Gate, StatusFailure, Unanswered};

/// The identity the ledger knows `identity` by.
pub(in crate::core::server) fn contributor_id(identity: &ModelClientIdentity) -> String {
    if let Some(context) = identity.human_context() {
        return context.user_id.to_string();
    }
    match identity.agent_id() {
        Some(agent) => agent.to_string(),
        None => identity.client_id.clone(),
    }
}

fn unanswered(error: Unanswered) -> ApiError {
    let status = match error {
        Unanswered::Undeclared => StatusCode::NOT_IMPLEMENTED,
        Unanswered::Failed(_) => StatusCode::SERVICE_UNAVAILABLE,
    };
    api_error_with_details(
        status,
        "contribution_ledger_unanswered",
        &error.to_string(),
        json!({}),
    )
}

/// `POST /v1/rewards/status` `{"alias"?, "agent_id"?}`, for this gateway's
/// console: the report `brama rewards status` prints.
pub(in crate::core::server) async fn report(
    Extension(client_identity): Extension<ModelClientIdentity>,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    require_brama_desktop(&client_identity)?;
    let parsed: Value = serde_json::from_slice(&body)
        .map_err(|error| api_error(StatusCode::BAD_REQUEST, &format!("invalid JSON: {error}")))?;
    let text = |key: &str| match parsed.get(key) {
        None => Ok(None),
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(Some(text.trim())),
        Some(other) => Err(api_error(StatusCode::BAD_REQUEST, &format!("`{key}` is not a name: {other}"))),
    };
    let (alias, agent) = (text("alias")?, text("agent_id")?);
    if agent.is_some() && alias.is_none() {
        return Err(api_error(StatusCode::BAD_REQUEST, "agent_id is answered for an alias: name `alias` too"));
    }
    match status(alias, agent).await {
        Ok(report) => Ok(Json(report)),
        Err(StatusFailure::Unanswered(error)) => Err(unanswered(error)),
        Err(StatusFailure::Unreadable(sentence)) => Err(api_error_with_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "contribution_declaration_unreadable",
            &sentence,
            json!({}),
        )),
    }
}

/// Admit `client_identity` to `model`, which its credential's model scope
/// does not name. An alias this gateway gates is admitted when the ledger
/// says the caller holds the entitlement to it; every other model is refused
/// as the scope refuses it.
pub(in crate::core::server) async fn admit_beyond_scope(
    client_identity: &ModelClientIdentity,
    model: &str,
) -> Result<(), ApiError> {
    let scope_refusal = || api_error(StatusCode::FORBIDDEN, &client_identity.model_refusal(model));
    match gated(model) {
        Ok(false) => return Err(scope_refusal()),
        Ok(true) => {}
        Err(error) => {
            return Err(api_error_with_details(
                StatusCode::SERVICE_UNAVAILABLE,
                "gated_aliases_unreadable",
                &format!("whether {model} is gated could not be read: {error}"),
                json!({}),
            ))
        }
    }
    let contributor = contributor_id(client_identity);
    match check_entitlement(&contributor, model).await.map_err(unanswered)? {
        Gate::Granted { entitlement_id } => {
            info!(event = "entitlement_admitted", contributor = %contributor, model, entitlement_id,
                "a gated alias admitted on the caller's entitlement");
            Ok(())
        }
        Gate::Denied { reason, message } => Err(api_error_with_details(
            StatusCode::FORBIDDEN,
            "entitlement_required",
            &format!("{}; {model} is served to holders of its entitlement: {message}", client_identity.model_refusal(model)),
            json!({"reason": reason, "contributor_id": contributor, "scope": model}),
        )),
    }
}
