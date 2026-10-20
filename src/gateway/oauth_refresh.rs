//! Refreshing a stored OAuth credential with its provider.
//!
//! This file owns the exchange itself: one bounded HTTP call, the grant it
//! returns, and writing that grant back into the credential. The three things
//! it needs to know are each their own module, because they change for
//! different reasons — `provider` is the per-provider table and credential
//! shape, `expiry` reads when an access token dies, and `refusal` decides
//! whether a refused refresh means the grant is dead or the network blinked.

mod expiry;
mod principal;
mod provider;
mod refusal;

use serde::Serialize;
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use crate::capability::Secret;
use crate::core::failure;
use wisent_errors::{Code, Failure};

use provider::{oauth_provider, oauth_refresh_token, patch_oauth_blob, zeroize_json_strings};
use refusal::{refresh_failure, rejection_failure};

// The surface `gateway` has always used, named one by one so the module's
// callers keep their exact paths and nothing else leaks with them.
pub(super) use expiry::{access_token_expiry_ms, needs_refresh};
pub(super) use principal::{stated_account, stated_account_text, GrantText};
pub(crate) use principal::jwt_claims;
pub(super) use provider::supports_refresh;
pub(super) use refusal::{classify_refusal, RefreshRefusal};

/// Whether `document` is a grant this module could renew for `provider`: it
/// has the provider's shape and carries a refresh token. The one check a
/// grant taken from a harness or handed over by a console must pass before
/// it is stored, so the sweep never finds a credential it cannot read.
pub fn renewable(document: &Value, provider: &str) -> bool {
    oauth_refresh_token(document, provider).is_some()
}

#[derive(Serialize)]
struct OAuthRefreshRequest<'a> {
    grant_type: &'static str,
    refresh_token: &'a str,
    client_id: &'static str,
}

/// One provider answer, held only as long as it takes to write it into the
/// credential. The fields are read by `provider::patch_oauth_blob`, which is
/// a child of this module and sees them without them being public.
struct RefreshGrant {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
}

impl Drop for RefreshGrant {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(token) = self.refresh_token.as_mut() {
            token.zeroize();
        }
        if let Some(token) = self.id_token.as_mut() {
            token.zeroize();
        }
    }
}

fn parse_refresh_grant(body: &Value) -> Option<RefreshGrant> {
    Some(RefreshGrant {
        access_token: body
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())?
            .to_owned(),
        refresh_token: body
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_owned),
        id_token: body
            .get("id_token")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(str::to_owned),
        // A stated lifetime of zero seconds states none.
        expires_in: body
            .get("expires_in")
            .and_then(Value::as_u64)
            .and_then(std::num::NonZeroU64::new)
            .map(std::num::NonZeroU64::get),
    })
}

/// Read the whole response before deciding what the provider refused.
/// An incomplete refusal cannot establish that the grant is invalid.
async fn response_body(
    response: &mut reqwest::Response,
    endpoint: &str,
) -> Result<Zeroizing<Vec<u8>>, Failure> {
    let mut encoded = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        refresh_failure(
            Code::InfraDown,
            format!(
                "OAuth refresh POST {endpoint}: HTTP {} body read failed after {} bytes: {}",
                response.status().as_u16(),
                encoded.len(),
                failure::error_chain(&error),
            ),
        )
    })? {
        encoded.extend_from_slice(&chunk);
    }
    Ok(encoded)
}

async fn request_refresh_grant(
    config: &provider::OAuthProvider,
    refresh_token: &str,
) -> Result<RefreshGrant, Failure> {
    // One client for every refresh. A fresh `Client` per call brings a fresh
    // connection pool with it and strands the previous one's sockets.
    static REFRESH_CLIENT: std::sync::LazyLock<Result<reqwest::Client, String>> =
        std::sync::LazyLock::new(|| {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|error| {
                    format!(
                        "OAuth refresh client configuration failed: {}",
                        failure::error_chain(&error),
                    )
                })
        });
    let client = REFRESH_CLIENT
        .as_ref()
        .map_err(|detail| refresh_failure(Code::Config, detail.as_str()))?;
    let parameters = OAuthRefreshRequest {
        grant_type: "refresh_token",
        refresh_token,
        client_id: config.client_id,
    };
    let request = client
        .post(config.token_endpoint)
        .header(reqwest::header::ACCEPT, "application/json");
    let request = match config.wire {
        provider::OAuthWire::Json => request.json(&parameters),
        provider::OAuthWire::Form => request.form(&parameters),
    }
    .build()
    .map_err(|error| {
        refresh_failure(
            Code::Config,
            format!(
                "OAuth refresh POST {} cannot be assembled: {}",
                config.token_endpoint,
                failure::error_chain(&error),
            ),
        )
    })?;
    let mut response = stado_wait::send_built(
        stado_wait::Kind::Network,
        "OAuth refresh",
        client.clone(),
        request,
    )
    .await
    .map_err(|error| {
        refresh_failure(
            Code::InfraDown,
            format!(
                "OAuth refresh POST {} failed: {}",
                config.token_endpoint,
                failure::error_chain(&error),
            ),
        )
    })?;
    // The body says which of `invalid_grant`, a revoked client or a throttle
    // the provider answered, so it travels with the failure.
    let status = response.status();
    let mut encoded = response_body(&mut response, config.token_endpoint).await?;
    if !status.is_success() {
        let body = String::from_utf8_lossy(&encoded);
        return Err(rejection_failure(status.as_u16(), &body));
    }
    let mut body: Value = serde_json::from_slice(&encoded).map_err(|error| {
        refresh_failure(
            Code::Unknown,
            format!(
                "OAuth refresh response from {} (HTTP {}) is not JSON: {error}",
                config.token_endpoint,
                status.as_u16(),
            ),
        )
    })?;
    encoded.zeroize();
    let grant = parse_refresh_grant(&body);
    zeroize_json_strings(&mut body);
    grant.ok_or_else(|| {
        refresh_failure(
            Code::Unknown,
            format!(
                "OAuth refresh response from {} (HTTP {}) has no nonempty access_token",
                config.token_endpoint,
                status.as_u16(),
            ),
        )
    })
}

pub(super) async fn refresh(
    secret: &Secret,
    provider: &str,
) -> Result<Zeroizing<Vec<u8>>, Failure> {
    // Every arm below describes credential material this deployment stored in a
    // shape the refresh cannot use, which is `config`: waiting does not change
    // it and the provider was never asked.
    let config = oauth_provider(provider)
        .ok_or_else(|| refresh_failure(Code::Config, "provider does not support OAuth refresh"))?;
    let raw = secret.expose_utf8().map_err(|error| {
        refresh_failure(
            Code::Config,
            format!("read stored OAuth credential for `{provider}` as UTF-8: {error}"),
        )
    })?;
    let mut blob: Value = serde_json::from_str(raw).map_err(|error| {
        refresh_failure(
            Code::Config,
            format!("parse stored OAuth credential for `{provider}` as JSON: {error}"),
        )
    })?;
    if !blob.is_object() {
        zeroize_json_strings(&mut blob);
        return Err(refusal::stored_document_failure(refusal::NOT_AN_OBJECT));
    }
    let refresh_token = match oauth_refresh_token(&blob, provider) {
        Some(token) => token,
        None => {
            zeroize_json_strings(&mut blob);
            return Err(refusal::stored_document_failure(refusal::NO_REFRESH_TOKEN));
        }
    };
    let result = async {
        let grant = request_refresh_grant(&config, &refresh_token).await?;
        if !patch_oauth_blob(&mut blob, provider, &grant, expiry::now_seconds()) {
            return Err(refresh_failure(
                Code::Config,
                "OAuth credential shape mismatch",
            ));
        }
        let fresh = Zeroizing::new(serde_json::to_vec(&blob).map_err(|error| {
            refresh_failure(
                Code::Config,
                format!("serialize refreshed OAuth credential for `{provider}`: {error}"),
            )
        })?);
        if fresh.is_empty() {
            return Err(refresh_failure(
                Code::Config,
                "refreshed OAuth credential is empty",
            ));
        }
        Ok(fresh)
    }
    .await;
    zeroize_json_strings(&mut blob);
    result
}
