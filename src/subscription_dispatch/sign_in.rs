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

mod account;
mod trajectory;
mod verdict;
mod worker_api;

use std::time::Duration;

use serde_json::{json, Value};

use crate::subscription_dispatch::pool;

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

/// Sign one provider account in through Weles, then prove the repair the way
/// the runbook does: by a refresh that answers `refreshed`.
///
/// Transport and dependency refusals -- an unknown provider, a missing reason,
/// no reachable Weles worker -- are `Err` so an automatic retry is not cooled
/// down. An account-mapping refusal is a completed failed verdict: the same
/// undeclared account cannot become correct on the next minute's sweep.
pub async fn sign_in_provider(options: SignInOptions) -> Result<Value, String> {
    let provider = options.provider.trim().to_string();
    let weles_provider = match provider.as_str() {
        "claude-code" => "claude",
        "codex" => "codex",
        "kimi" => "kimi",
        "" => return Err("a provider is required".into()),
        other => {
            return Err(format!(
                "Weles signs in claude-code, codex and kimi; `{other}` is not one of them"
            ))
        }
    };
    let reason = options.reason.trim().to_string();
    if reason.is_empty() {
        return Err("--reason must say why this sign-in is being run".into());
    }

    let base = worker_api_base().await?;
    let token = worker_api_token()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(transport_timeout_seconds()))
        .build()
        .map_err(|error| format!("cannot build an HTTP client: {error}"))?;

    // Everything that can be known before a browser opens is checked here,
    // because the cost of finding out afterwards is one real sign-in into the
    // wrong account.
    let health = read_health(&client, &base).await?;
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
        .map_err(|error| format!("Weles worker API refused the sign-in request: {error}"))?;
    let status = response.status().as_u16();
    let answer: Value = response.json().await.map_err(|error| {
        format!(
            "Weles sign-in response at {base}/reauth (HTTP {status}) is not valid JSON: {error}"
        )
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
        Some(subscription_id) => {
            pool::refresh_subscription(&provider, subscription_id, &reason).await?
        }
        None => pool::refresh_provider(&provider, &reason).await?,
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
