//! Refreshing a stored OAuth credential with its provider.
//!
//! This file owns the exchange itself: one bounded HTTP call, the grant it
//! returns, and writing that grant back into the credential. The three things
//! it needs to know are each their own module, because they change for
//! different reasons — `provider` is the per-provider table and credential
//! shape, `expiry` reads when an access token dies, and `refusal` decides
//! whether a refused refresh means the grant is dead or the network blinked.

mod expiry;
mod provider;
mod refusal;

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

use crate::capability::Secret;
use wisent_errors::{Code, Failure};

use provider::{oauth_provider, oauth_refresh_token, patch_oauth_blob, zeroize_json_strings};
use refusal::{refresh_failure, rejection_failure};

// The surface `gateway` has always used, named one by one so the module's
// callers keep their exact paths and nothing else leaks with them.
pub(super) use expiry::{access_token_expiry_ms, expires_within, needs_refresh};
pub(super) use provider::supports_refresh;
pub(super) use refusal::{classify_refusal, RefreshRefusal};

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

fn refresh_timeout() -> Duration {
    Duration::from_secs("15".parse().expect("valid OAuth refresh timeout"))
}

fn max_response_bytes() -> usize {
    "65536".parse().expect("valid OAuth response limit")
}

fn max_credential_bytes() -> usize {
    "8192".parse().expect("valid credential size limit")
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
        expires_in: body
            .get("expires_in")
            .and_then(Value::as_u64)
            .filter(|seconds| *seconds > u64::default()),
    })
}

/// Read a refused response's body under the same bound the success path uses. A
/// provider that answers with a megabyte of HTML does not get to fill the log.
async fn bounded_error_body(response: &mut reqwest::Response) -> String {
    let mut text = String::new();
    while let Ok(Some(chunk)) = response.chunk().await {
        if text.len().saturating_add(chunk.len()) > max_response_bytes() {
            break;
        }
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    text
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
                .timeout(refresh_timeout())
                .build()
                .map_err(|_| "OAuth refresh client configuration failed".to_owned())
        });
    let client = REFRESH_CLIENT
        .clone()
        .map_err(|detail| refresh_failure(Code::Config, detail))?;
    let parameters = OAuthRefreshRequest {
        grant_type: "refresh_token",
        refresh_token,
        client_id: config.client_id,
    };
    let request = client
        .post(config.token_endpoint)
        .header(reqwest::header::ACCEPT, "application/json");
    let mut response = match config.wire {
        provider::OAuthWire::Json => request.json(&parameters),
        provider::OAuthWire::Form => request.form(&parameters),
    }
    .send()
    .await
    .map_err(|error| {
        let code = if error.is_timeout() {
            Code::Timeout
        } else {
            Code::InfraDown
        };
        refresh_failure(code, "OAuth refresh transport failure")
    })?;
    // The status alone was all this returned, and the status alone is what a
    // day went into supplementing by hand. The body says which of
    // `invalid_grant`, a revoked client or a throttle it was, so it travels
    // with the failure.
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = bounded_error_body(&mut response).await;
        return Err(rejection_failure(status, &body));
    }
    if response
        .content_length()
        .is_some_and(|length| length > max_response_bytes() as u64)
    {
        return Err(refresh_failure(
            Code::Unknown,
            "OAuth refresh response is too large",
        ));
    }
    let mut encoded = Zeroizing::new(Vec::new());
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| refresh_failure(Code::InfraDown, "OAuth refresh response read failed"))?
    {
        if encoded.len().saturating_add(chunk.len()) > max_response_bytes() {
            return Err(refresh_failure(
                Code::Unknown,
                "OAuth refresh response is too large",
            ));
        }
        encoded.extend_from_slice(&chunk);
    }
    let mut body: Value = serde_json::from_slice(&encoded)
        .map_err(|_| refresh_failure(Code::Unknown, "OAuth refresh response is not JSON"))?;
    encoded.zeroize();
    let grant = parse_refresh_grant(&body);
    zeroize_json_strings(&mut body);
    grant
        .ok_or_else(|| refresh_failure(Code::Unknown, "OAuth refresh response has no access token"))
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
    let raw = secret
        .expose_utf8()
        .map_err(|_| refresh_failure(Code::Config, "OAuth credential is not UTF-8"))?;
    let mut blob: Value = serde_json::from_str(raw)
        .map_err(|_| refresh_failure(Code::Config, "OAuth credential is not JSON"))?;
    if !blob.is_object() {
        zeroize_json_strings(&mut blob);
        return Err(refresh_failure(
            Code::Config,
            "OAuth credential is not an object",
        ));
    }
    let refresh_token = match oauth_refresh_token(&blob, provider) {
        Some(token) => token,
        None => {
            zeroize_json_strings(&mut blob);
            return Err(refresh_failure(
                Code::Config,
                "OAuth credential has no refresh token",
            ));
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
        let fresh = Zeroizing::new(serde_json::to_vec(&blob).map_err(|_| {
            refresh_failure(
                Code::Config,
                "refreshed OAuth credential is not serializable",
            )
        })?);
        if fresh.is_empty() || fresh.len() > max_credential_bytes() {
            return Err(refresh_failure(
                Code::Config,
                "refreshed OAuth credential size is invalid",
            ));
        }
        Ok(fresh)
    }
    .await;
    zeroize_json_strings(&mut blob);
    result
}
