//! The account behind a human caller: the agent id its subscriptions are
//! stored under, which provider a credential may be banked for, and the Weles
//! login item a donation may name.

use axum::http::StatusCode;

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::refusal::{api_error, ApiError};

pub(in crate::core::server) fn account_agent_id(
    identity: &ModelClientIdentity,
) -> Result<String, ApiError> {
    let context = identity.human_context().ok_or_else(|| {
        api_error(
            StatusCode::FORBIDDEN,
            &format!(
                "client {} carries no signed-in Wisent user, and this request is answered only for one",
                identity.client_id
            ),
        )
    })?;
    Ok(format!("user-{}", context.user_id.simple()))
}

pub(in crate::core::server) async fn account_agent_for_route(
    identity: &ModelClientIdentity,
    route: &str,
) -> Result<Option<String>, ApiError> {
    let Some(context) = identity.human_context() else {
        return Ok(None);
    };
    let agent_id = format!("user-{}", context.user_id.simple());
    let Some(provider) = crate::providers::adapter::provider_id_from_route(route) else {
        return Ok(None);
    };
    let entries = crate::gateway::broker::routing_subscriptions(&agent_id).map_err(|error| {
        api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("route {route}: no live subscription; discovery failed: {error}"),
        )
    })?;
    for entry in entries {
        if !entry.provider.eq_ignore_ascii_case(provider) || entry.status != "active" {
            continue;
        }
        let retired = crate::journal::is_retired(&entry.id).map_err(|error| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("route {route}: {error}"),
            )
        })?;
        if !retired {
            return Ok(Some(agent_id));
        }
    }
    Ok(None)
}

/// The provider a credential may be banked for: `Ok(None)` when the value
/// names none Brama supports. A model catalogue that cannot be read is its own
/// refusal: read as `None`, it told the caller a catalogued provider was
/// unsupported while the real cause was the catalogue.
pub(in crate::core::server) async fn account_credential_provider(
    value: Option<&str>,
) -> Result<Option<String>, ApiError> {
    let provider = match value.map(str::trim) {
        Some("claude_code") => "claude-code",
        Some(provider) if !provider.is_empty() => provider,
        _ => return Ok(None),
    };
    if provider == "local-openai" {
        return Ok(None);
    }
    if crate::providers::adapter::provider(provider).is_some() {
        return Ok(Some(provider.to_string()));
    }
    let catalogue = crate::subscription_dispatch::model_catalog::snapshot()
        .await
        .map_err(|error| {
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("provider {provider} is not built in, and the model catalogue that lists the others cannot be read: {error}"),
            )
        })?;
    Ok(catalogue
        .providers
        .get(provider)
        .filter(|provider| provider.executable())
        .map(|provider| provider.id.clone()))
}

pub(super) fn donation_login_item(value: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty()
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "login_item must be ASCII letters, digits, hyphens, underscores or dots",
        ));
    }
    Ok(Some(value.to_owned()))
}
