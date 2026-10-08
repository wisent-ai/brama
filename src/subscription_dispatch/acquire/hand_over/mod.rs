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
    // The pool's accounts as Weles resolves them from the vault it signs
    // them in from: the machine the harness runs on holds no vault of its
    // own, and the account each row names is the one Weles will complete the
    // harness's authorization as. A subscription that does not resolve is
    // reported with Weles's own refusal.
    let listed = weles::accounts(provider).await?;
    let mut rows = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for member in listed["accounts"].as_array().into_iter().flatten() {
        let (Some(subscription_id), Some(account)) = (
            member["subscription_id"].as_str(),
            member["account"].as_str(),
        ) else {
            rows.push(json!({"result": "unresolved", "detail": format!(
                "Weles listed an account row without a subscription id or account: {member}")}));
            continue;
        };
        let account = account.trim().to_lowercase();
        if crate::journal::is_retired(subscription_id) || !seen.insert(account.clone()) {
            continue;
        }
        if held.contains(&account) {
            rows.push(
                json!({"subscription_id": subscription_id, "account": account, "result": "held"}),
            );
            continue;
        }
        rows.push(
            sign_in(
                provider,
                harness,
                harness_provider,
                subscription_id,
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
        // Pool subscriptions of any provider Weles could not resolve to an
        // account, each with its refusal: none of them can be handed over.
        "unresolved": listed["errors"],
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
