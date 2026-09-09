//! The account behind a human caller: the agent id its subscriptions are
//! stored under, which provider a credential may be banked for, and the Weles
//! login item a donation may name.

use axum::http::StatusCode;

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::refusal::{api_error, ApiError};

pub(in crate::core::server) fn account_agent_id(
    identity: &ModelClientIdentity,
) -> Result<String, ApiError> {
    let context = identity
        .human_context()
        .ok_or_else(|| api_error(StatusCode::FORBIDDEN, "forbidden"))?;
    Ok(format!("user-{}", context.user_id.simple()))
}

pub(in crate::core::server) async fn account_agent_for_route(
    identity: &ModelClientIdentity,
    route: &str,
) -> Option<String> {
    let agent_id = account_agent_id(identity).ok()?;
    let provider = crate::providers::adapter::provider_id_from_route(route)?;
    crate::gateway::broker::list_subscriptions(&agent_id)
        .await
        .into_iter()
        .any(|entry| {
            entry.provider.eq_ignore_ascii_case(provider)
                && entry.status == "active"
                && !crate::journal::is_retired(&entry.id)
        })
        .then_some(agent_id)
}

pub(in crate::core::server) async fn account_credential_provider(
    value: Option<&str>,
) -> Option<String> {
    let provider = match value.map(str::trim) {
        Some("claude_code") => "claude-code",
        Some(provider) if !provider.is_empty() => provider,
        _ => return None,
    };
    if provider == "local-openai" {
        return None;
    }
    if crate::providers::adapter::provider(provider).is_some() {
        return Some(provider.to_string());
    }
    crate::subscription_dispatch::model_catalog::snapshot()
        .await
        .ok()?
        .providers
        .get(provider)
        .filter(|provider| provider.executable())
        .map(|provider| provider.id.clone())
}

pub(super) fn donation_login_item(value: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty()
        || value.len() > 160
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "login_item must contain 1..160 ASCII letters, digits, hyphens, underscores or dots",
        ));
    }
    Ok(Some(value.to_owned()))
}
