//! Banking a credential into the pool and retiring one out of it: the two
//! membership changes, and the persisted identity each of them writes.

use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::core::server::admission::identity::valid_agent_id;
use crate::core::server::refusal::{api_error, ApiError};

use super::account::{account_credential_provider, donation_login_item};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DonateSubscriptionRequest {
    pub(super) provider: Option<String>,
    pub(super) label: Option<String>,
    pub(super) api_key: Option<String>,
    pub(super) login_item: Option<String>,
    pub(super) subscription_id: Option<String>,
    pub(super) account: Option<String>,
}

pub(super) async fn create_subscription(
    agent_id: String,
    request: DonateSubscriptionRequest,
) -> Result<Json<Value>, ApiError> {
    if !valid_agent_id(&agent_id) {
        return Err(api_error(StatusCode::BAD_REQUEST, "invalid agent id"));
    }
    let provider = account_credential_provider(request.provider.as_deref())
        .await
        .ok_or_else(|| {
            api_error(
                StatusCode::BAD_REQUEST,
                "provider must name a supported remote API or subscription provider",
            )
        })?;
    let login_item = donation_login_item(request.login_item.as_deref())?;
    let api_key = request.api_key.as_deref().unwrap_or("");
    if api_key.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "api_key must not be empty",
        ));
    }
    let account = request
        .account
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let subscription_id = if let Some(requested_id) = request
        .subscription_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        let owned = crate::gateway::broker::discover_subscriptions(&agent_id)
            .await
            .map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?
            .into_iter()
            .find(|entry| entry.id == requested_id)
            .ok_or_else(|| api_error(StatusCode::NOT_FOUND, "subscription not found"))?;
        if owned.provider != provider {
            return Err(api_error(
                StatusCode::CONFLICT,
                "selected subscription belongs to a different provider",
            ));
        }
        if let (Some(expected), Some(stated)) = (owned.account.as_deref(), account) {
            if !expected.eq_ignore_ascii_case(stated) {
                return Err(api_error(
                    StatusCode::CONFLICT,
                    "selected subscription belongs to a different account",
                ));
            }
        }
        requested_id.to_owned()
    } else {
        let account = account.ok_or_else(|| {
            api_error(
                StatusCode::BAD_REQUEST,
                "account email address is required when adding a subscription",
            )
        })?;
        format!(
            "{}-{}",
            crate::gateway::broker::slug(&provider),
            crate::gateway::broker::slug(&account.to_lowercase())
        )
    };
    crate::gateway::broker::put_donated_credential(
        &agent_id,
        &provider,
        &subscription_id,
        api_key,
        login_item.as_deref(),
        account,
    )
    .await
    .map_err(|refusal| match refusal {
        crate::gateway::broker::DonationRefusal::Unusable(detail) => {
            api_error(StatusCode::BAD_REQUEST, &detail)
        }
        crate::gateway::broker::DonationRefusal::MappingConflict(detail) => {
            api_error(StatusCode::CONFLICT, &detail)
        }
        crate::gateway::broker::DonationRefusal::Unwritable(detail) => {
            api_error(StatusCode::INTERNAL_SERVER_ERROR, &detail)
        }
    })?;
    crate::gateway::broker::donated_add(
        &agent_id,
        &subscription_id,
        &provider,
        request.label.as_deref(),
        login_item.as_deref(),
    )
    .map_err(|message| api_error(StatusCode::INTERNAL_SERVER_ERROR, &message))?;
    Ok(Json(json!({
        "subscription": {
            "id": subscription_id,
            "provider": provider,
            "agent_id": agent_id,
            "status": "active",
            "label": request.label,
            "account": account,
            "login_item": login_item,
        }
    })))
}

pub(super) async fn retire_managed_subscription(
    agent_id: String,
    subscription_id: String,
) -> Result<Json<Value>, ApiError> {
    let owned = crate::gateway::broker::owned_subscriptions(&agent_id)
        .await
        .map_err(|detail| api_error(StatusCode::SERVICE_UNAVAILABLE, &detail))?
        .into_iter()
        .find(|entry| entry.id == subscription_id);
    let Some(owned) = owned else {
        return Err(api_error(StatusCode::NOT_FOUND, "subscription not found"));
    };
    crate::journal::retire(&subscription_id);
    // Retirement is recorded in the ledger as well as the journal, so a row can
    // say `disabled` with the instant and the reason it happened rather than
    // leaving a reader to infer a retirement from an absence.
    crate::subscription_dispatch::usage::record_credential_disabled(
        &subscription_id,
        &owned.provider,
        "retired by its owning agent",
    );
    crate::gateway::broker::donated_remove(&subscription_id)
        .map_err(|detail| api_error(StatusCode::CONFLICT, &detail))?;
    crate::gateway::broker::remove_donated_credential(&owned.provider, &subscription_id)
        .map_err(|detail| api_error(StatusCode::CONFLICT, &detail))?;
    Ok(Json(json!({"ok": true})))
}
