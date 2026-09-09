//! What Wisent Identity says about a person's bearer: which user it is, and
//! whether that user was authorized for the organization the request names. A
//! user is not an identity here until both answers arrive.

use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::identity::{HumanOrganizationContext, OrganizationRole};
use super::{IdentityResolutionError, WisentIdentityAnswer, WISENT_ORGANIZATION_HEADER};

const WISENT_SUPABASE_URL: &str = "https://alvaewvbyxpgwdpugnxy.supabase.co";
const WISENT_SUPABASE_ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImFsdmFld3ZieXhwZ3dkcHVnbnh5Iiwicm9sZSI6ImFub24iLCJpYXQiOjE3ODEzOTc5NDcsImV4cCI6MjA5Njk3Mzk0N30.xkkJ36ZTwtqyVZLFju0vc9S25grTuKbj9ILKlsXdUPA";

#[derive(Deserialize)]
struct OrganizationAuthorization {
    user_id: uuid::Uuid,
    organization_id: uuid::Uuid,
    role: OrganizationRole,
}

/// Where the Wisent Identity authority answers.
///
/// The anon key this gateway presents was already deployment-configurable
/// while the origin it presents it to was compiled in, which left the human
/// half of every identity decision unprovable anywhere except against the
/// production project. A deployment overriding this is choosing its own
/// identity authority, exactly as `WC_SKARBIEC_URL` chooses the workload one;
/// unset, it is canonical Wisent Supabase and nothing changes.
fn wisent_identity_origin() -> String {
    std::env::var("BRAMA_WISENT_AUTH_URL")
        .ok()
        .map(|configured| configured.trim().trim_end_matches('/').to_owned())
        .filter(|configured| !configured.is_empty())
        .unwrap_or_else(|| WISENT_SUPABASE_URL.to_owned())
}

pub(super) async fn ask_wisent_identity(bearer: &str) -> WisentIdentityAnswer {
    let anon_key = std::env::var("BRAMA_WISENT_AUTH_ANON_KEY")
        .unwrap_or_else(|_| WISENT_SUPABASE_ANON_KEY.to_string());
    if anon_key.trim().is_empty() {
        return WisentIdentityAnswer::Unavailable;
    }
    let client = match crate::providers::adapter::control_client() {
        Ok(client) => client,
        Err(_) => return WisentIdentityAnswer::Unavailable,
    };
    let response = match client
        .get(format!("{}/auth/v1/user", wisent_identity_origin()))
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
        Ok(bytes) if bytes.len() <= 64 * 1024 => bytes,
        _ => return WisentIdentityAnswer::Unavailable,
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
    let anon_key = std::env::var("BRAMA_WISENT_AUTH_ANON_KEY")
        .unwrap_or_else(|_| WISENT_SUPABASE_ANON_KEY.to_string());
    if anon_key.trim().is_empty() {
        return Err(IdentityResolutionError::UpstreamUnavailable);
    }
    let client = crate::providers::adapter::control_client()
        .map_err(|_| IdentityResolutionError::UpstreamUnavailable)?;
    let response = client
        .post(format!(
            "{}/rest/v1/rpc/authorize_organization",
            wisent_identity_origin()
        ))
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
    if bytes.len() > 64 * 1024 {
        return Err(IdentityResolutionError::UpstreamUnavailable);
    }
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
