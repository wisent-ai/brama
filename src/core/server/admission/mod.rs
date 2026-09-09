//! May this request be served at all, and as whom?
//!
//! Three questions, asked in this order and nowhere else: was the hop
//! protected ([`transport`]), does the presented bearer name a client
//! ([`ingress`], then [`authority`] for anything this process was not started
//! with), and — where a resource belongs to one agent — did the caller sign
//! this exact body for that agent. Everything downstream is handed the
//! [`identity::ModelClientIdentity`] these produce and asks no further.

pub(in crate::core::server) mod authority;
pub(in crate::core::server) mod identity;
pub(in crate::core::server) mod ingress;
pub(in crate::core::server) mod transport;

use axum::extract::State;
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tracing::{info, warn};

use crate::core::server::refusal::{api_error, ApiError};
use crate::subscription_dispatch::authenticate_agent;

use authority::{identity_from_authority, IdentityResolutionError};
use identity::ModelClientIdentity;
use ingress::ModelIngressAuth;

pub(in crate::core::server) fn presented_bearer(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    let value = value.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        return None;
    }
    Some(token)
}

fn exact_agent_header(headers: &HeaderMap, expected: &str) -> bool {
    let mut values = headers.get_all("x-agent-id").iter();
    values
        .next()
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
        && values.next().is_none()
}

pub(in crate::core::server) fn has_caller_auth_headers(headers: &HeaderMap) -> bool {
    ["x-agent-id", "x-agent-timestamp", "x-agent-signature"]
        .iter()
        .any(|name| headers.contains_key(*name))
}

/// The subscription capability paths a model-scoped bearer may reach.
///
/// Every workload identity the authority resolves is model-scoped, because
/// that authority always answers with `allowed_models: Some(routes)`, and a
/// human identity carries an empty allowlist. Leaving one of these paths out
/// of the allowlist makes that capability unreachable by construction for the
/// only callers it exists for: on charless-mac-mini the Weles renewal
/// trajectory read `list subscriptions -> 401` on every tick while the pool it
/// was there to refill stayed empty, and the refusal named neither the path
/// nor the reason. The four per-audience usage refreshes plan usage replaces
/// were never in this list at all, so the one audience that signs for itself
/// could not reach its own usage on any of them.
///
/// Reaching the path is not being served by it. The handler still resolves the
/// caller's identity, requires a signature over the exact body for an
/// agent-scoped write, and narrows the answer to what that identity owns.
fn is_subscription_capability_path(path: &str) -> bool {
    matches!(path, "/v1/subscription-pool" | "/v1/plan-usage")
}

pub(in crate::core::server) async fn require_model_bearer(
    State(auth): State<ModelIngressAuth>,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    let identity = match auth.identity_for(request.headers()) {
        Some(identity) => identity,
        // The table is a copy of the vault taken at boot: it cannot expire, it
        // cannot be revoked, and a client registered since this process started
        // is absent from it. Ask the authority that issued the credential
        // instead of refusing on the strength of a snapshot.
        None => match identity_from_authority(request.headers()).await {
            Ok(identity) => identity,
            Err(IdentityResolutionError::InvalidOrganizationHeader) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "a valid X-Wisent-Organization-ID header is required",
                )
                .into_response()
            }
            Err(IdentityResolutionError::Forbidden) => {
                return api_error(StatusCode::FORBIDDEN, "forbidden").into_response()
            }
            Err(IdentityResolutionError::Unauthorized) => {
                warn!(
                    path = %request.uri().path(),
                    event = "model_bearer_unrecognized",
                    "no boot-table client and no authority answer matched the presented bearer"
                );
                return api_error(StatusCode::UNAUTHORIZED, "unauthorized").into_response();
            }
            Err(IdentityResolutionError::UpstreamUnavailable) => {
                return api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "identity upstream unavailable",
                )
                .into_response()
            }
        },
    };
    if identity.allowed_models.is_some()
        && !is_subscription_capability_path(request.uri().path())
        && !matches!(
            request.uri().path(),
            // Every inference and discovery path a model-scoped bearer may
            // reach. The three chat formats are one workflow, so a client
            // allowed to complete a chat is allowed to complete the same chat
            // in the dialect it speaks; the model allowlist itself is enforced
            // per request, further in.
            "/v1/chat/completions"
                | "/v1/messages"
                | "/v1/responses"
                | "/v1/embeddings"
                | "/v1/moderations"
                | "/v1/models"
        )
    {
        warn!(
            client_id = %identity.client_id,
            path = %request.uri().path(),
            event = "model_scoped_bearer_path_refused",
            "a model-scoped credential may not reach this path"
        );
        return api_error(StatusCode::FORBIDDEN, "forbidden").into_response();
    }
    if let Some(agent_id) = identity.agent_id() {
        if has_caller_auth_headers(request.headers())
            && !exact_agent_header(request.headers(), agent_id)
        {
            warn!(client_id = %identity.client_id, agent_id, "bearer identity does not match signed agent");
            return api_error(StatusCode::FORBIDDEN, "forbidden").into_response();
        }
    }
    if let Some(context) = identity.human_context().cloned() {
        info!(
            client_id = %identity.client_id,
            user_id = %context.user_id,
            organization_id = %context.organization_id,
            role = ?context.role,
            "human organization identity authorized"
        );
        request.extensions_mut().insert(context);
    }
    request.extensions_mut().insert(identity);
    next.run(request).await
}

/// Authenticate the signed caller and, for agent-scoped resources, bind the
/// caller identity to the exact path identity. Request bodies are verified as
/// received so subscription mutations cannot substitute an unsigned donor.
pub(in crate::core::server) async fn authorize_caller(
    client_identity: &ModelClientIdentity,
    headers: &axum::http::HeaderMap,
    raw_body: &[u8],
    target_agent_id: Option<&str>,
) -> Result<String, ApiError> {
    let caller = authenticate_agent(headers, raw_body)
        .await
        // The response stays deliberately blank -- an unauthenticated caller
        // learns nothing -- but the operator was learning nothing either. A
        // missing header, a clock outside the skew window, a secret the gateway
        // could not redeem and a genuinely wrong signature all arrived as one
        // word, and telling them apart from the outside is not possible: the
        // caller sees 401 whether the fault is its own or this side's.
        .map_err(|reason| {
            warn!(%reason, event = "caller_auth_rejected", "agent authentication refused");
            api_error(StatusCode::UNAUTHORIZED, "unauthorized")
        })?;
    if client_identity
        .agent_id()
        .is_some_and(|bound| bound != caller.as_str())
        || target_agent_id.is_some_and(|target| target != caller.as_str())
    {
        return Err(api_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(caller)
}
