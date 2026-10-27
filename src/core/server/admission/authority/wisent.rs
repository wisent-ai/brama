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
/// deployment that names none cannot resolve a person, one that names only
/// half of it is incomplete, and a value that is not Unicode is refused by
/// name; each says so.
fn wisent_identity() -> Result<(String, String), String> {
    let url = crate::environment::declared_setting("BRAMA_WISENT_AUTH_URL")?
        .map(|value| value.trim_end_matches('/').to_owned());
    let anon_key = crate::environment::declared_setting("BRAMA_WISENT_AUTH_ANON_KEY")?;
    match (url, anon_key) {
        (Some(url), Some(anon_key)) => Ok((url, anon_key)),
        (None, None) => Err(
            "BRAMA_WISENT_AUTH_URL and BRAMA_WISENT_AUTH_ANON_KEY are unset; no Wisent Identity authority is declared".into(),
        ),
        (Some(_), None) => Err(
            "the Wisent Identity declaration is incomplete: BRAMA_WISENT_AUTH_URL is set but BRAMA_WISENT_AUTH_ANON_KEY is unset".into(),
        ),
        (None, Some(_)) => Err(
            "the Wisent Identity declaration is incomplete: BRAMA_WISENT_AUTH_ANON_KEY is set but BRAMA_WISENT_AUTH_URL is unset".into(),
        ),
    }
}

pub(super) async fn ask_wisent_identity(bearer: &str) -> WisentIdentityAnswer {
    let (origin, anon_key) = match wisent_identity() {
        Ok(declared) => declared,
        Err(cause) => {
            return WisentIdentityAnswer::Unavailable(format!("{cause}; no person can be resolved"))
        }
    };
    let user = format!("{origin}/auth/v1/user");
    let client = match crate::providers::adapter::control_client() {
        Ok(client) => client,
        Err(error) => {
            return WisentIdentityAnswer::Unavailable(format!(
                "cannot build the Wisent Identity client: {error}"
            ))
        }
    };
    let response = match stado_wait::until(
        stado_wait::Kind::Network,
        "ask Wisent Identity who the bearer is",
        &user,
        client
            .get(&user)
            .header("apikey", &anon_key)
            .bearer_auth(bearer)
            .send(),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => return WisentIdentityAnswer::Unavailable(format!("GET {user}: {error}")),
    };
    let status = response.status();
    if !status.is_success() && !status.is_server_error() && status != StatusCode::TOO_MANY_REQUESTS
    {
        return WisentIdentityAnswer::Rejected;
    }
    let answer: Value =
        match super::response::read_json(response, "GET", &user, &[bearer, &anon_key]).await {
            Ok(answer) => answer,
            Err(error) => return WisentIdentityAnswer::Unavailable(error),
        };
    let Some(user_id) = answer.get("id").and_then(Value::as_str) else {
        return WisentIdentityAnswer::Unavailable(format!("GET {user} answered a user with no id"));
    };
    match uuid::Uuid::parse_str(user_id) {
        Ok(user_id) => WisentIdentityAnswer::Resolved(user_id),
        Err(error) => WisentIdentityAnswer::Unavailable(format!(
            "GET {user} answered an id that is not a UUID ({user_id}): {error}"
        )),
    }
}

fn organization_unavailable(cause: String) -> IdentityResolutionError {
    tracing::warn!(event = "organization_authority_unavailable", %cause);
    IdentityResolutionError::UpstreamUnavailable(format!(
        "the Wisent organization authority did not answer: {cause}"
    ))
}

pub(super) async fn authorize_organization(
    bearer: &str,
    expected_user_id: uuid::Uuid,
    expected_organization_id: uuid::Uuid,
) -> Result<HumanOrganizationContext, IdentityResolutionError> {
    let (origin, anon_key) = wisent_identity().map_err(|cause| {
        organization_unavailable(format!("{cause}; no organization can be authorized"))
    })?;
    let authorize = format!("{origin}/rest/v1/rpc/authorize_organization");
    let client = crate::providers::adapter::control_client().map_err(|error| {
        organization_unavailable(format!(
            "cannot build the Wisent Identity organization client: {error}"
        ))
    })?;
    let request = client
        .post(&authorize)
        .header("apikey", &anon_key)
        .header("Accept", "application/vnd.pgrst.object+json")
        .header(
            WISENT_ORGANIZATION_HEADER,
            expected_organization_id.to_string(),
        )
        .bearer_auth(bearer)
        .json(&json!({"target_org_id": expected_organization_id}));
    let response = stado_wait::until(
        stado_wait::Kind::Network,
        "ask Wisent Identity to authorize the organization",
        &authorize,
        request.send(),
    )
    .await
    .map_err(|error| organization_unavailable(format!("POST {authorize}: {error}")))?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED {
        return Err(IdentityResolutionError::Unauthorized);
    }
    if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_ACCEPTABLE {
        return Err(IdentityResolutionError::Forbidden);
    }
    let authorization: OrganizationAuthorization =
        super::response::read_json(response, "POST", &authorize, &[bearer, &anon_key])
            .await
            .map_err(organization_unavailable)?;
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
