//! Signing a Claude account in by hand: the operator's own browser, one
//! pasted code, and a grant stored exactly where a Weles sign-in would put it.
//!
//! Weles drives a browser on a dedicated host and needs that host, its worker
//! credential and a login row in Skarbiec. On 2026-09-12 none of those helped:
//! every Claude subscription in the pool held no credential, and the only
//! account that worked was signed into `omp` on the operator's laptop, where
//! `omp auth-broker login anthropic` had done what this does - open
//! `claude.ai/oauth/authorize` with PKCE, let the person log in, and take the
//! code back from a `localhost` redirect or a paste. Brama could not, so the
//! gateway every product is told to use had nothing to answer with while the
//! harness beside it did.
//!
//! The flow is the harness's flow, parameter for parameter: same client id,
//! same scopes, same loopback callback, same `code#state` paste, same token
//! endpoint. A grant that came from a different flow would be a different
//! kind of credential, and the refresh path would then have to know which.

mod proof;

use std::time::Duration;

use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::gateway::broker;

pub use proof::finish;

/// Everything the provider's authorize page needs, and the verifier the code
/// exchange proves it with. Built once per sign-in; the verifier never leaves
/// this process.
pub struct AuthorizationRequest {
    pub url: String,
    pub state: String,
    verifier: Zeroizing<String>,
    provider: String,
    subscription_id: String,
}

impl AuthorizationRequest {
    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn subscription_id(&self) -> &str {
        &self.subscription_id
    }
}

#[derive(Serialize)]
struct CodeExchange<'a> {
    grant_type: &'static str,
    code: &'a str,
    state: &'a str,
    client_id: &'static str,
    redirect_uri: &'static str,
    code_verifier: &'a str,
}

/// What one manual sign-in came to.
#[derive(Debug, Serialize)]
pub struct ManualSignIn {
    pub provider: String,
    pub subscription_id: String,
    pub account: Option<String>,
    pub result: &'static str,
    pub detail: String,
}

/// The provider's own OAuth constants for a manual sign-in: authorize page,
/// token endpoint, client id, scopes and the loopback redirect the client id
/// is registered for.
struct ManualProvider {
    authorize_url: &'static str,
    token_endpoint: &'static str,
    client_id: &'static str,
    scopes: &'static [&'static str],
    redirect_uri: &'static str,
}

fn manual_provider(provider: &str) -> Option<ManualProvider> {
    match provider {
        "claude-code" => Some(ManualProvider {
            authorize_url: "https://claude.ai/oauth/authorize",
            token_endpoint: "https://api.anthropic.com/v1/oauth/token",
            client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
            scopes: &[
                "org:create_api_key",
                "user:profile",
                "user:inference",
                "user:sessions:claude_code",
                "user:mcp_servers",
                "user:file_upload",
            ],
            // The port the client id is registered for; the browser lands
            // here whether or not anything listens, and the operator pastes
            // what it shows.
            redirect_uri: "http://localhost:54545/callback",
        }),
        _ => None,
    }
}

/// Bytes the provider's PKCE verifier and state are drawn from: 32 random
/// bytes, base64url without padding, as RFC 7636 describes.
const RANDOM_BYTES: usize = 32;

fn random_token() -> String {
    let mut bytes = [0u8; RANDOM_BYTES];
    getrandom(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn getrandom(buffer: &mut [u8]) {
    use std::io::Read;
    // /dev/urandom on every host this runs on; a failure here is a broken
    // machine, and a sign-in with a guessable verifier is worse than none.
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(buffer))
        .expect(
            "the host has no readable /dev/urandom; refusing to sign in with a guessable verifier",
        );
}

/// The page the operator opens. Nothing is contacted yet.
pub fn begin(provider: &str, subscription_id: &str) -> Result<AuthorizationRequest, String> {
    let config = manual_provider(provider).ok_or_else(|| {
        format!("a manual sign-in is defined for claude-code; `{provider}` is not it")
    })?;
    if subscription_id.trim().is_empty() {
        return Err("an exact subscription id is required".into());
    }
    let verifier = Zeroizing::new(random_token());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let state = random_token();
    let mut url =
        url::Url::parse(config.authorize_url).map_err(|error| format!("authorize url: {error}"))?;
    url.query_pairs_mut()
        .append_pair("code", "true")
        .append_pair("client_id", config.client_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", config.redirect_uri)
        .append_pair("scope", &config.scopes.join(" "))
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);
    Ok(AuthorizationRequest {
        url: url.to_string(),
        state,
        verifier,
        provider: provider.to_owned(),
        subscription_id: subscription_id.trim().to_owned(),
    })
}

/// The code and state out of whatever the operator pasted: the bare
/// `code#state` the provider shows, or the whole redirect URL.
pub fn parse_pasted(input: &str, expected_state: &str) -> Result<(String, String), String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("nothing was pasted".into());
    }
    let (code, state) = if let Ok(url) = url::Url::parse(input) {
        let mut code = None;
        let mut state = None;
        for (key, value) in url.query_pairs() {
            match &*key {
                "code" => code = Some(value.into_owned()),
                "state" => state = Some(value.into_owned()),
                _ => {}
            }
        }
        (
            code.ok_or("the pasted URL carries no `code`")?,
            state.ok_or("the pasted URL carries no `state`")?,
        )
    } else if let Some((code, state)) = input.split_once('#') {
        (code.to_owned(), state.to_owned())
    } else {
        return Err("paste the `code#state` the page shows, or the whole redirect URL".into());
    };
    if state != expected_state {
        return Err("the pasted state is not the one this sign-in started with; open the URL again and paste what that page shows".into());
    }
    if code.is_empty() {
        return Err("the pasted code is empty".into());
    }
    Ok((code, state))
}

fn exchange_timeout() -> Duration {
    Duration::from_secs("30".parse().expect("valid exchange timeout"))
}

fn max_response_bytes() -> usize {
    "65536".parse().expect("valid response limit")
}

fn millis_per_second() -> i64 {
    "1000".parse().expect("valid milliseconds per second")
}

/// Exchange the pasted code for a grant, store it as this subscription's
/// credential in the shape the refresh path reads, and say what happened.
pub async fn complete(request: AuthorizationRequest, pasted: &str) -> Result<ManualSignIn, String> {
    let config = manual_provider(&request.provider)
        .ok_or_else(|| format!("no manual sign-in for {}", request.provider))?;
    let (code, state) = parse_pasted(pasted, &request.state)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(exchange_timeout())
        .build()
        .map_err(|error| format!("token exchange client: {error}"))?;
    let response = client
        .post(config.token_endpoint)
        .header(reqwest::header::ACCEPT, "application/json")
        .json(&CodeExchange {
            grant_type: "authorization_code",
            code: &code,
            state: &state,
            client_id: config.client_id,
            redirect_uri: config.redirect_uri,
            code_verifier: &request.verifier,
        })
        .send()
        .await
        .map_err(|error| format!("the token exchange did not reach the provider: {error}"))?;
    let status = response.status().as_u16();
    if response
        .content_length()
        .is_some_and(|length| length > max_response_bytes() as u64)
    {
        return Err("the provider's answer is too large to be a grant".into());
    }
    let body = Zeroizing::new(
        response
            .bytes()
            .await
            .map_err(|error| format!("reading the provider's answer: {error}"))?
            .to_vec(),
    );
    if !(200..300).contains(&status) {
        let text = String::from_utf8_lossy(&body);
        return Ok(ManualSignIn {
            provider: request.provider,
            subscription_id: request.subscription_id,
            account: None,
            result: "failed",
            detail: format!(
                "the provider refused the code with HTTP {status}: {}",
                text.trim()
            ),
        });
    }
    let grant: Value = serde_json::from_slice(&body)
        .map_err(|_| "the provider's answer is not JSON".to_owned())?;
    let access = grant
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or("the provider's answer carries no access token")?;
    let refresh = grant
        .get("refresh_token")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or("the provider's answer carries no refresh token; Brama cannot keep a grant it cannot renew")?;
    let expires_in = grant
        .get("expires_in")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let now = chrono::Utc::now().timestamp();
    let account = grant
        .get("account")
        .and_then(|account| account.get("email_address"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    // The exact shape `oauth_refresh::provider` reads for claude-code, so the
    // sweep that keeps this grant alive sees nothing unusual about it.
    let credential = Zeroizing::new(
        json!({
            "claudeAiOauth": {
                "accessToken": access,
                "refreshToken": refresh,
                "expiresAt": (now + expires_in as i64) * millis_per_second(),
                "scopes": config.scopes,
            }
        })
        .to_string(),
    );
    broker::put_subscription_credential(
        &request.subscription_id,
        &request.provider,
        credential.as_bytes(),
    )
    .await
    .map_err(|detail| format!("the grant was issued but could not be stored: {detail}"))?;
    Ok(ManualSignIn {
        provider: request.provider,
        subscription_id: request.subscription_id,
        detail: format!(
            "the operator signed {} in and its grant is stored; Brama will refresh it from now on",
            account.as_deref().unwrap_or("the account")
        ),
        account,
        result: "signed_in",
    })
}
