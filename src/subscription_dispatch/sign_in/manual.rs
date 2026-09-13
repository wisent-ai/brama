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
//! And when the harness already holds the grant, [`omp`] takes it from there
//! instead of asking the person to log in a second time.

mod exchange;
pub mod grant;
pub mod omp;

use base64::Engine;
use serde::Serialize;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub use exchange::complete;
pub use grant::{adopt, Grant, Origin};

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

    fn verifier(&self) -> &str {
        &self.verifier
    }
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
