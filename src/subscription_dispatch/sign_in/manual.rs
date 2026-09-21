//! Signing a Claude account in: the provider's own OAuth flow, run by Brama
//! itself, ending in a grant that belongs to this gateway and nobody else.
//!
//! The flow is the one every coding harness runs, parameter for parameter:
//! same client id, same scopes, same loopback callback, same `code#state`
//! paste, same token endpoint. Run here it mints a NEW pair, which is the
//! whole point. Brama used to be able to take the pair a harness on the
//! machine already held, and that was removed on 2026-09-20: a provider
//! issues one pair per sign-in and revokes it when a second holder refreshes,
//! so the borrowed copy cost the operator the session they were working in,
//! twice, and it made a gateway depend on a workstation staying in the fleet
//! and on that workstation running omp at all. A gateway signs itself in.
//!
//! Weles drives this same flow headlessly on the gateway's own host
//! (`subscription sign-in`); this module is the path a person can run from a
//! terminal, and the one Weles's exchange ends in.

mod exchange;
pub mod grant;

use base64::Engine;
use serde::Serialize;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub use exchange::complete;
pub use grant::{adopt, Origin};

/// Milliseconds in one second, for the expiry arithmetic the exchange does.
pub(super) const MILLIS_PER_SECOND: i64 = 1000;

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

/// The scopes the harness's Claude client asks for, which every Claude grant
/// document Brama writes carries.
pub(super) fn claude_scopes() -> &'static [&'static str] {
    manual_provider("claude-code")
        .map(|config| config.scopes)
        .unwrap_or_default()
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
