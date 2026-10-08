//! Signing every pool account a coding-agent harness on this machine does not
//! hold yet into that harness.
//!
//! An account bought by an acquisition is in Brama's pool, but the agents
//! that spent the old accounts run in a harness with its own credential
//! store. Each account the pool holds and the harness lacks is signed in
//! through the harness's own login (`harness`), with Weles completing the
//! authorization as that account, and the harness's account list is read
//! again afterwards: an account counts as handed over only when the harness
//! itself lists it.

pub mod harness;

use serde_json::{json, Value};

use super::weles;
use crate::subscription_dispatch::sign_in::worker::progress::Progress;
pub use harness::Harness;

/// The verdict of one hand-over: one row per pool account, and `ok` only when
/// the harness lists every account the pool holds.
pub async fn hand_over(
    provider: &str,
    harness: Harness,
    progress: Option<&Progress>,
) -> Result<Value, String> {
    let harness_provider = harness.provider_id(provider).ok_or_else(|| {
        format!(
            "{} has no account of Brama's provider `{provider}` to sign in; claude-code is \
             handed to omp as anthropic",
            harness.name()
        )
    })?;
    weles::weles_provider(provider).ok_or_else(|| {
        format!("Weles authorizes claude-code harnesses; `{provider}` is not one of them")
    })?;
    let Some(held) = harness::held_accounts(harness, harness_provider).await? else {
        return Ok(json!({
            "provider": provider,
            "harness": harness.name(),
            "ok": true,
            "installed": false,
            "detail": format!("{} is not installed on this machine, so no account is signed into it here", harness.name()),
            "accounts": [],
        }));
    };
    let members = crate::gateway::broker::list_all_subscriptions()
        .await
        .map_err(|error| {
            format!("the Skarbiec subscription inventory could not be read: {error}")
        })?;
    let mut rows = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for member in members
        .iter()
        .filter(|entry| entry.provider == provider && !crate::journal::is_retired(&entry.id))
    {
        let Some(account) = member
            .account
            .as_deref()
            .map(str::trim)
            .filter(|account| !account.is_empty())
            .map(str::to_lowercase)
        else {
            rows.push(
                json!({"subscription_id": member.id, "result": "unattributed",
                "detail": "the member declares no account in brama:account:, so which harness \
                    account it is cannot be read; `brama subscription attribute` records it"}),
            );
            continue;
        };
        if !seen.insert(account.clone()) {
            continue;
        }
        if held.contains(&account) {
            rows.push(json!({"subscription_id": member.id, "account": account, "result": "held"}));
            continue;
        }
        rows.push(
            sign_in(
                provider,
                harness,
                harness_provider,
                &member.id,
                &account,
                progress,
            )
            .await,
        );
    }
    let Some(after) = harness::held_accounts(harness, harness_provider).await? else {
        return Err(format!(
            "{} was uninstalled from this machine while accounts were being signed into it",
            harness.name()
        ));
    };
    for row in &mut rows {
        if row["result"] == json!("handed_over") {
            let listed = row["account"]
                .as_str()
                .is_some_and(|account| after.contains(account));
            if !listed {
                row["result"] = json!("failed");
                row["detail"] = json!(format!(
                    "the harness login ended successfully but `{} usage accounts` does not list \
                     the account",
                    harness.name()
                ));
            }
        }
    }
    let ok = rows
        .iter()
        .all(|row| row["result"] == json!("held") || row["result"] == json!("handed_over"));
    Ok(json!({
        "provider": provider,
        "harness": harness.name(),
        "ok": ok,
        "installed": true,
        "accounts": rows,
    }))
}

async fn sign_in(
    provider: &str,
    harness: Harness,
    harness_provider: &str,
    subscription_id: &str,
    account: &str,
    progress: Option<&Progress>,
) -> Value {
    let failed = |detail: String| {
        json!({"subscription_id": subscription_id, "account": account, "result": "failed",
            "detail": detail})
    };
    let login = match harness::Login::start(harness, harness_provider).await {
        Ok(login) => login,
        Err(detail) => return failed(detail),
    };
    let exchange =
        match weles::authorize(provider, subscription_id, &login.authorize_url, progress).await {
            Ok(exchange) => exchange,
            Err(detail) => return failed(detail),
        };
    if let Some(detail) = exchange.transport_failure {
        return failed(detail);
    }
    let Some(answer) = exchange.answer else {
        return failed("Weles answered the authorization without a result".to_string());
    };
    let Some(redirect) = answer["redirect_url"]
        .as_str()
        .filter(|_| answer["ok"] == json!(true))
    else {
        return failed(format!(
            "Weles did not complete the authorization as {account}: {}",
            answer["failure"]
        ));
    };
    match login.finish(redirect).await {
        Ok(()) => json!({"subscription_id": subscription_id, "account": account,
            "result": "handed_over", "run_id": answer["run_id"], "weles": exchange.observed}),
        Err(detail) => failed(detail),
    }
}
