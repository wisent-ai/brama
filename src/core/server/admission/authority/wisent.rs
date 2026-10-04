//! What Wisent Identity says about a person's bearer: which user it is, and
//! whether that user was authorized for the organization the request names. A
//! user is not an identity here until both answers arrive.

use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::identity::{HumanOrganizationContext, OrganizationRole};
use super::{IdentityResolutionError, WisentIdentityAnswer, WISENT_ORGANIZATION_HEADER};

#[derive(Deserialize)]
struct OrganizationAuthorization {
    user_id: uuid::Uuid,
    organization_id: uuid::Uuid,
    role: OrganizationRole,
}

/// Where the Wisent Identity authority answers, and the anon key it is asked
/// with: `BRAMA_WISENT_AUTH_URL` and `BRAMA_WISENT_AUTH_ANON_KEY`, which the
/// Stado service catalog sets for the gateway. No authority is compiled in: a
/// deployment that names none cannot resolve a person, and says so.
fn wisent_identity() -> Option<(String, String)> {
    let configured = |name: &str| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_owned())
            .filter(|value| !value.is_empty())
    };
    Some((
        configured("BRAMA_WISENT_AUTH_URL")?,
        configured("BRAMA_WISENT_AUTH_ANON_KEY")?,
    ))
}

pub(super) async fn ask_wisent_identity(bearer: &str) -> WisentIdentityAnswer {
    let Some((origin, anon_key)) = wisent_identity() else {
        tracing::warn!(
            event = "wisent_identity_unconfigured",
            "BRAMA_WISENT_AUTH_URL or BRAMA_WISENT_AUTH_ANON_KEY is unset; no person can be resolved"
        );
        return WisentIdentityAnswer::Unavailable;
    };
    let client = match crate::providers::adapter::control_client() {
        Ok(client) => client,
        Err(_) => return WisentIdentityAnswer::Unavailable,
    };
    let response = match client
        .get(format!("{origin}/auth/v1/user"))
        .header("apikey", &anon_key)
        .bearer_auth(bearer)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return WisentIdentityAnswer::Unavailable,
    };
    if response.status().is_server_error() || response.status() == StatusCode::TOO_MANY_REQUESTS {
        return WisentIdentityAnswer::Unavailable;
    }
    if !response.status().is_success() {
        return WisentIdentityAnswer::Rejected;
    }
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => return WisentIdentityAnswer::Unavailable,
    };
    let answer: Value = match serde_json::from_slice(&bytes) {
        Ok(answer) => answer,
        Err(_) => return WisentIdentityAnswer::Unavailable,
    };
    let Some(user_id) = answer.get("id").and_then(Value::as_str) else {
        return WisentIdentityAnswer::Unavailable;
    };
    match uuid::Uuid::parse_str(user_id) {
        Ok(user_id) => WisentIdentityAnswer::Resolved(user_id),
        Err(_) => WisentIdentityAnswer::Unavailable,
    }
}

pub(super) async fn authorize_organization(
    bearer: &str,
    expected_user_id: uuid::Uuid,
    expected_organization_id: uuid::Uuid,
) -> Result<HumanOrganizationContext, IdentityResolutionError> {
    let Some((origin, anon_key)) = wisent_identity() else {
        return Err(IdentityResolutionError::UpstreamUnavailable);
    };
    let client = crate::providers::adapter::control_client()
        .map_err(|_| IdentityResolutionError::UpstreamUnavailable)?;
    let response = client
        .post(format!("{origin}/rest/v1/rpc/authorize_organization"))
        .header("apikey", anon_key)
        .header("Accept", "application/vnd.pgrst.object+json")
        .header(
            WISENT_ORGANIZATION_HEADER,
            expected_organization_id.to_string(),
        )
        .bearer_auth(bearer)
        .json(&json!({"target_org_id": expected_organization_id}))
        .send()
        .await
        .map_err(|_| IdentityResolutionError::UpstreamUnavailable)?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED {
        return Err(IdentityResolutionError::Unauthorized);
    }
    if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_ACCEPTABLE {
        return Err(IdentityResolutionError::Forbidden);
    }
    if !status.is_success() {
        return Err(IdentityResolutionError::UpstreamUnavailable);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| IdentityResolutionError::UpstreamUnavailable)?;
    let authorization: OrganizationAuthorization =
        serde_json::from_slice(&bytes).map_err(|_| IdentityResolutionError::UpstreamUnavailable)?;
    if authorization.user_id != expected_user_id
        || authorization.organization_id != expected_organization_id
    {
        return Err(IdentityResolutionError::Forbidden);
    }
    Ok(HumanOrganizationContext {
        user_id: authorization.user_id,
        organization_id: authorization.organization_id,
        role: authorization.role,
    })
}
