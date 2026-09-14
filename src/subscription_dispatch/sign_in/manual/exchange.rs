//! The code the operator pasted, exchanged at the provider's token endpoint
//! for a grant, and handed on to be stored and proved like every other.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroizing;

use super::grant::{adopt, claude_document, Origin};
use super::{manual_provider, parse_pasted, AuthorizationRequest, ManualSignIn};

#[derive(Serialize)]
struct CodeExchange<'a> {
    grant_type: &'static str,
    code: &'a str,
    state: &'a str,
    client_id: &'static str,
    redirect_uri: &'static str,
    code_verifier: &'a str,
}

fn exchange_timeout() -> Duration {
    Duration::from_secs("30".parse().expect("valid exchange timeout"))
}

fn max_response_bytes() -> u64 {
    "65536".parse().expect("valid response limit")
}

fn millis_per_second() -> i64 {
    "1000".parse().expect("valid milliseconds per second")
}

/// Exchange the pasted code for a grant, store it as this subscription's
/// credential in the shape the refresh path reads, and say what happened.
pub async fn complete(
    request: AuthorizationRequest,
    pasted: &str,
    reason: &str,
) -> Result<ManualSignIn, String> {
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
            code_verifier: request.verifier(),
        })
        .send()
        .await
        .map_err(|error| format!("the token exchange did not reach the provider: {error}"))?;
    let status = response.status().as_u16();
    if response
        .content_length()
        .is_some_and(|length| length > max_response_bytes())
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
    let issued: Value = serde_json::from_slice(&body)
        .map_err(|_| "the provider's answer is not JSON".to_owned())?;
    let token = |name: &str| {
        issued
            .get(name)
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty())
            .map(|token| Zeroizing::new(token.to_owned()))
    };
    let expires_in = issued
        .get("expires_in")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let access = token("access_token").ok_or("the provider's answer carries no access token")?;
    let refresh = token("refresh_token").ok_or(
        "the provider's answer carries no refresh token; Brama cannot keep a grant it cannot renew",
    )?;
    let expires_at_ms = (chrono::Utc::now().timestamp() + expires_in) * millis_per_second();
    let account = issued
        .get("account")
        .and_then(|account| account.get("email_address"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    adopt(
        &request.provider,
        &request.subscription_id,
        claude_document(&access, &refresh, expires_at_ms),
        account,
        Origin::PastedCode,
        reason,
    )
    .await
}
