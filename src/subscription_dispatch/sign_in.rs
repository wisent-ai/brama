//! One operator-driven sign-in of a provider subscription account.
//!
//! A grant the provider disowned is only replaced by a real sign-in. Weles owns
//! that sign-in, drives it in its own browser on its own host, and exposes it
//! as `POST /reauth` on its worker API. Brama and Weles receive the same
//! `brama-weles-reauth` bearer independently from Skarbiec at service startup:
//! Brama presents it and Weles accepts it only on the reauthentication route.
//! No shared host, environment file, helper, or copied secret connects them.
//!
//! Nothing here opens a browser and nothing here reads or prints credential
//! material. What the sign-in mints is written by Weles into the vault; the
//! proof this command reports is the same proof the runbook names: the refresh
//! that follows answers `refreshed`.

pub mod blocked;

mod account;
mod trajectory;
mod verdict;
mod worker_api;

use std::time::Duration;

use serde_json::{json, Value};

use crate::subscription_dispatch::pool;

pub use blocked::{declared_account, Blocked, SignInError};

use account::{resolve_login_item, subscription_mismatch};
use verdict::{verdict, FAILED, SIGNED_IN};
use worker_api::{read_health, transport_timeout_seconds, worker_api_base, worker_api_token};

/// What one sign-in was asked to do.
pub struct SignInOptions {
    /// The provider whose account should be signed in (`codex`, `claude-code`,
    /// `kimi`).
    pub provider: String,
    /// The exact Weles sign-in row to drive, or `None` to use the account Weles
    /// explicitly declares primary for the provider.
    pub login_item: Option<String>,
    /// Exact subscription whose stored grant this sign-in replaces. Automatic
    /// renewal always supplies it; older provider-wide CLI calls may not.
    pub subscription_id: Option<String>,
    /// Why this sign-in is being run; recorded in the journal beside the
    /// verdict.
    pub reason: String,
    /// How long Weles may spend driving the browser, in milliseconds. A
    /// sign-in walks a real SSO and a consent screen, so the budget is
    /// minutes, not seconds.
    pub login_timeout_ms: u64,
}

/// Weles's own name for a provider whose accounts it signs in, or nothing for
/// a provider it does not.
///
/// One list, read by the sign-in itself and by every surface that has to say
/// whether an account is repaired automatically at all: an API key is not
/// signed in by anybody, and reporting one as awaiting a sign-in is how a
/// healthy account comes to look broken.
pub fn weles_provider(provider: &str) -> Option<&'static str> {
    match provider.trim() {
        "claude-code" => Some("claude"),
        "codex" => Some("codex"),
        "kimi" => Some("kimi"),
        _ => None,
    }
}

/// Sign one provider account in through Weles, then prove the repair the way
/// the runbook does: by a refresh that answers `refreshed`.
///
/// A refusal that is a missing declaration -- no Weles account for this
/// subscription, a Weles that holds none or several, a Weles this gateway
/// cannot reach -- is [`SignInError::Blocked`], carrying the word and the
/// sentence the sweep logs and the pool document shows. A dependency refusal,
/// such as an unknown provider or a refresh that would not run, says nothing
/// about the fleet's declarations and stays a plain one.
pub async fn sign_in_provider(options: SignInOptions) -> Result<Value, SignInError> {
    let provider = options.provider.trim().to_string();
    let weles_provider = match weles_provider(&provider) {
        Some(weles_provider) => weles_provider,
        None if provider.is_empty() => {
            return Err(SignInError::Dependency("a provider is required".into()))
        }
        None => {
            return Err(SignInError::Dependency(format!(
                "Weles signs in claude-code, codex and kimi; `{provider}` is not one of them"
            )))
        }
    };
    let reason = options.reason.trim().to_string();
    if reason.is_empty() {
        return Err(SignInError::Dependency(
            "--reason must say why this sign-in is being run".into(),
        ));
    }

    // Weles owns every sign-in, so a Weles this gateway cannot reach is a
    // deployment that can repair no credential at all. That is a statement
    // about this fleet and it gets its own word, not a swallowed blip.
    let base = worker_api_base()
        .await
        .map_err(|detail| Blocked::WelesUnreachable { detail })?;
    let token = worker_api_token().map_err(|detail| Blocked::WelesUnreachable { detail })?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(transport_timeout_seconds()))
        .build()
        .map_err(|error| {
            SignInError::Dependency(format!("cannot build an HTTP client: {error}"))
        })?;

    // Everything that can be known before a browser opens is checked here,
    // because the cost of finding out afterwards is one real sign-in into the
    // wrong account.
    let health = read_health(&client, &base)
        .await
        .map_err(|detail| Blocked::WelesUnreachable { detail })?;
    let selecting_declared_primary = options
        .login_item
        .as_deref()
        .is_none_or(|item| item.trim().is_empty());
    let login_item = resolve_login_item(
        &health,
        weles_provider,
        options.login_item.as_deref().map(str::trim),
    )?;
    if let Some(expected) = options.subscription_id.as_deref() {
        if let Some(detail) =
            subscription_mismatch(&health, &login_item, expected, selecting_declared_primary)
        {
            return Ok(verdict(
                Some(expected),
                &provider,
                &login_item,
                &reason,
                FAILED,
                0,
                "",
                detail,
                Value::Null,
            ));
        }
    }

    let body = json!({
        "provider": weles_provider,
        "login_item": login_item,
        "subscription_id": options.subscription_id.as_deref(),
        "timeout_ms": options.login_timeout_ms,
    });
    let response = client
        .post(format!("{base}/reauth"))
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .map_err(|error| Blocked::WelesUnreachable {
            detail: format!("Weles worker API refused the sign-in request: {error}"),
        })?;
    let status = response.status().as_u16();
    let answer: Value = response.json().await.map_err(|error| {
        SignInError::Dependency(format!(
            "Weles sign-in response at {base}/reauth (HTTP {status}) is not valid JSON: {error}"
        ))
    })?;

    let account = trajectory::signed_in_account(&answer);
    if let Some(detail) = trajectory::refusal(&answer, status, &login_item) {
        return Ok(verdict(
            options.subscription_id.as_deref(),
            &provider,
            &login_item,
            &reason,
            FAILED,
            status,
            &account,
            detail,
            Value::Null,
        ));
    }

    // The runbook's own proof: a sign-in replaced the grant, and the refresh
    // that follows answers `refreshed`. The refresh writes its own journal
    // record beside this one, exactly as if the operator had run it.
    let refresh = match options.subscription_id.as_deref() {
        Some(subscription_id) => pool::refresh_subscription(&provider, subscription_id, &reason)
            .await
            .map_err(SignInError::Dependency)?,
        None => pool::refresh_provider(&provider, &reason)
            .await
            .map_err(SignInError::Dependency)?,
    };
    let refreshed = refresh.get("result").and_then(Value::as_str) == Some("refreshed");
    let detail = if refreshed {
        format!(
            "Weles signed `{login_item}` in and the exact subscription refresh obtained a credential"
        )
    } else {
        format!(
            "Weles signed `{login_item}` in, but the refresh that followed obtained nothing: {}",
            refresh
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("no detail was reported")
        )
    };
    Ok(verdict(
        options.subscription_id.as_deref(),
        &provider,
        &login_item,
        &reason,
        if refreshed { SIGNED_IN } else { FAILED },
        status,
        &account,
        detail,
        refresh,
    ))
}
