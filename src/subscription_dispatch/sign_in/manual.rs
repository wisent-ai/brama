//! Signing a Claude account in: the provider's own OAuth flow, run by Brama
//! itself, ending in a grant that belongs to this gateway and nobody else.
//!
//! The flow is the one every coding harness runs, parameter for parameter:
//! same client id, same scopes, same manual-paste redirect (the provider's
//! own page shows the code, so no port on this machine has to listen), same
//! `code#state` paste, same token endpoint. Run here it mints a NEW pair,
//! which is the whole point. Brama does not take the pair a harness on the
//! machine already holds: a provider issues one pair per sign-in and revokes it when
//! a second holder refreshes, so a borrowed copy costs the operator the
//! session they are working in, and it makes a gateway depend on a
//! workstation staying in the fleet and running a particular harness at
//! all. A gateway signs itself in.
//!
//! Weles drives this same flow headlessly on the gateway's own host
//! (`subscription sign-in --by weles`); this module is `--by hand`, the path a person can run from a
//! terminal, and the one Weles's exchange ends in.

mod exchange;
pub mod grant;

use base64::Engine;
use serde::Serialize;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub use exchange::complete;
pub use grant::{adopt, Origin};

/// Milliseconds in one second, for the expiry arithmetic the exchange does:
/// https://www.bipm.org/en/measurement-units/si-prefixes (milli = 10^-3).
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
/// token endpoint, client id, scopes and the provider-hosted redirect that
/// displays the code to paste.
struct ManualProvider {
    authorize_url: &'static str,
    token_endpoint: &'static str,
    client_id: &'static str,
    scopes: &'static [&'static str],
    redirect_uri: &'static str,
}

/// The harness's Claude client, the one manual sign-in Brama defines, and
/// the one declaration of it a refresh reads too. Its addresses are the ones
/// Claude Code 2.1.296 carries in its own OAuth configuration
/// (CLAUDE_AI_AUTHORIZE_URL, TOKEN_URL, MANUAL_REDIRECT_URL), so a grant
/// Brama asks for and renews is asked where the client it stands in for asks.
const CLAUDE_CODE: ManualProvider = ManualProvider {
    authorize_url: "https://claude.com/cai/oauth/authorize",
    token_endpoint: "https://platform.claude.com/v1/oauth/token",
    client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
    scopes: &[
        "org:create_api_key",
        "user:profile",
        "user:inference",
        "user:sessions:claude_code",
        "user:mcp_servers",
        "user:file_upload",
    ],
    // The client's manual-paste redirect: with `code=true` the provider's
    // own page shows the `code#state` the operator pastes, so nothing on the
    // signing-in machine has to listen for the browser.
    redirect_uri: "https://platform.claude.com/oauth/code/callback",
};

fn manual_provider(provider: &str) -> Option<ManualProvider> {
    match provider {
        "claude-code" => Some(CLAUDE_CODE),
        _ => None,
    }
}

/// The scopes the harness's Claude client asks for, which every Claude grant
/// document Brama writes carries.
pub(super) fn claude_scopes() -> &'static [&'static str] {
    CLAUDE_CODE.scopes
}

/// The client id of the harness's Claude client, which a refresh of a grant
/// this client issued must name.
pub(crate) fn claude_client_id() -> &'static str {
    CLAUDE_CODE.client_id
}

/// The token endpoint of the harness's Claude client, where a grant it issued
/// is both redeemed and refreshed.
pub(crate) fn claude_token_endpoint() -> &'static str {
    CLAUDE_CODE.token_endpoint
}

/// Bytes the provider's PKCE verifier and state are drawn from, base64url
/// without padding, as recommended in
/// https://www.rfc-editor.org/rfc/rfc7636#section-4.1 (a 32-octet sequence).
const RANDOM_BYTES: usize = 32;

fn random_token() -> Result<String, String> {
    let mut bytes: [u8; RANDOM_BYTES] = [Default::default(); RANDOM_BYTES];
    getrandom(&mut bytes)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

/// Fill `buffer` from /dev/urandom. A host that cannot give it refuses the
/// sign-in with the OS error: a sign-in with a guessable verifier is worse
/// than none, and the gateway serving other callers keeps running.
fn getrandom(buffer: &mut [u8]) -> Result<(), String> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(buffer))
        .map_err(|error| {
            format!("the PKCE verifier cannot be drawn: /dev/urandom could not be read ({error}); the sign-in is refused rather than run with a guessable verifier")
        })
}

/// Why a manual sign-in could not start: the request names what the manual
/// sign-in does not serve, or this host could not start it. The two are
/// answered differently (a refused request against a failing host), so they
/// are kept apart rather than joined into one sentence.
pub enum BeginRefused {
    /// The caller asked for a provider or subscription it cannot sign in.
    Request(String),
    /// This host could not draw the PKCE verifier or state, or build the page.
    Host(String),
}

impl std::fmt::Display for BeginRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request(said) | Self::Host(said) => formatter.write_str(said),
        }
    }
}

impl From<BeginRefused> for String {
    fn from(refused: BeginRefused) -> Self {
        refused.to_string()
    }
}

/// The page the operator opens. Nothing is contacted yet.
pub fn begin(provider: &str, subscription_id: &str) -> Result<AuthorizationRequest, BeginRefused> {
    let config = manual_provider(provider).ok_or_else(|| {
        BeginRefused::Request(format!(
            "a manual sign-in is defined for claude-code; `{provider}` is not it"
        ))
    })?;
    if subscription_id.trim().is_empty() {
        return Err(BeginRefused::Request(
            "an exact subscription id is required".into(),
        ));
    }
    let verifier = Zeroizing::new(random_token().map_err(BeginRefused::Host)?);
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let state = random_token().map_err(BeginRefused::Host)?;
    let mut url = url::Url::parse(config.authorize_url)
        .map_err(|error| BeginRefused::Host(format!("authorize url: {error}")))?;
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
