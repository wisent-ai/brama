//! `brama subscription sync`: every grant a harness on this machine holds
//! joins the pool, without anyone naming it.
//!
//! On 2026-09-17 the pool refused every Claude call - `all bounded
//! 'claude-code' credentials unavailable for agent` - while the operator was
//! talking to Claude on the same machine on an account no pool member named.
//! The account was added by hand, one `import` at a time, and the operator's
//! answer was that this is the opposite of how it should work: the pool has
//! to see every account the machine holds by itself. This is that sweep. It
//! lists what the harnesses hold, derives one stable member id per account,
//! skips every id the pool already has - Brama refreshes those itself, and a
//! harness's copy is stale the moment it did - and imports the rest through
//! the same path `import` takes, one verdict per grant.

use std::io::Read;

use serde::Serialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use brama::subscription_dispatch::sign_in::manual::{self, HeldGrant, ManualSignIn};

use super::harness;

/// What the sweep did with one held grant.
#[derive(Serialize)]
pub(crate) struct SyncRow {
    pub harness: String,
    pub provider: String,
    pub account: Option<String>,
    pub subscription_id: Option<String>,
    /// `imported`, `present`, `unnamed` or `failed`.
    pub result: &'static str,
    pub detail: String,
}

/// The pool member one held account stands for: the same id every sweep,
/// so a second sweep finds it present rather than importing it again.
pub(crate) fn member_id(provider: &str, account: &str) -> String {
    format!(
        "brama-sub-held-{}-{}",
        brama::gateway::broker::slug(provider),
        brama::gateway::broker::slug(account)
    )
}

/// The ids the pool holds now, read where the sweep will write: through the
/// gateway with the console's bearer, or this process's own vault.
async fn present_ids(gateway: Option<&str>, bearer: &str) -> Result<Vec<String>, String> {
    match gateway {
        Some(gateway) => {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .map_err(|error| format!("gateway client: {error}"))?;
            let response = client
                .get(format!(
                    "{}/v1/subscription-pool",
                    gateway.trim_end_matches('/')
                ))
                .bearer_auth(bearer)
                .send()
                .await
                .map_err(|error| format!("the gateway {gateway} did not answer: {error}"))?;
            let status = response.status().as_u16();
            let body: Value = response
                .json()
                .await
                .map_err(|error| format!("the gateway's pool report is not JSON: {error}"))?;
            if !(200..300).contains(&status) {
                let message = body
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("no reason given");
                return Err(format!(
                    "the gateway refused the pool report with HTTP {status}: {message}"
                ));
            }
            Ok(body["subscriptions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| row["id"].as_str().map(str::to_owned))
                .collect())
        }
        None => Ok(brama::gateway::broker::list_all_subscriptions()
            .await?
            .into_iter()
            .map(|entry| entry.id)
            .collect()),
    }
}

/// Sweep every held grant into the pool. The console's bearer is read from
/// stdin once when a gateway is named.
pub(crate) async fn sync(
    reason: &str,
    home: Option<&str>,
    gateway: Option<&str>,
) -> Result<Vec<SyncRow>, String> {
    if reason.trim().is_empty() {
        return Err("--reason must say why this sync is being run".into());
    }
    let mut bearer = Zeroizing::new(String::new());
    if gateway.is_some() {
        std::io::stdin()
            .read_to_string(&mut bearer)
            .map_err(|error| format!("reading the console bearer from stdin: {error}"))?;
        if bearer.trim().is_empty() {
            return Err(
                "--gateway needs the console's bearer on stdin, and stdin was empty".into(),
            );
        }
    }
    let bearer = bearer.trim();
    let held = manual::held(&harness::home(home), None)?;
    let present = present_ids(gateway, bearer).await?;
    let mut rows = Vec::with_capacity(held.len());
    for grant in held {
        rows.push(sweep_one(grant, &present, reason, gateway, bearer).await);
    }
    Ok(rows)
}

async fn sweep_one(
    grant: HeldGrant,
    present: &[String],
    reason: &str,
    gateway: Option<&str>,
    bearer: &str,
) -> SyncRow {
    let harness_name = grant.harness.name().to_owned();
    let provider = grant.provider.to_owned();
    let Some(account) = grant.account.clone() else {
        return SyncRow {
            harness: harness_name,
            provider,
            account: None,
            subscription_id: None,
            result: "unnamed",
            detail:
                "the harness records no account for this grant, so no pool member can stand for it"
                    .into(),
        };
    };
    let subscription_id = member_id(&provider, &account);
    if present.iter().any(|id| *id == subscription_id) {
        return SyncRow {
            harness: harness_name,
            provider,
            account: Some(account),
            subscription_id: Some(subscription_id),
            result: "present",
            detail: "the pool already holds this account; Brama refreshes its grant itself".into(),
        };
    }
    let verdict: Result<ManualSignIn, String> = match gateway {
        Some(gateway) => {
            harness::import_through_with(
                gateway,
                bearer,
                &provider,
                &subscription_id,
                reason,
                grant,
            )
            .await
        }
        None => harness::import_here(&provider, &subscription_id, reason, grant).await,
    };
    match verdict {
        Ok(verdict) if verdict.result == "signed_in" => SyncRow {
            harness: harness_name,
            provider,
            account: Some(account),
            subscription_id: Some(subscription_id),
            result: "imported",
            detail: verdict.detail,
        },
        Ok(verdict) => SyncRow {
            harness: harness_name,
            provider,
            account: Some(account),
            subscription_id: Some(subscription_id),
            result: "failed",
            detail: verdict.detail,
        },
        Err(detail) => SyncRow {
            harness: harness_name,
            provider,
            account: Some(account),
            subscription_id: Some(subscription_id),
            result: "failed",
            detail,
        },
    }
}

/// Print the sweep and exit unsuccessfully when any grant failed to join.
pub(crate) fn finish(outcome: Result<Vec<SyncRow>, String>, json: bool) {
    match outcome {
        Ok(rows) => {
            let failed = rows.iter().filter(|row| row.result == "failed").count();
            if json {
                crate::cli::print_json(&json!({
                    "ok": failed == 0,
                    "grants": rows,
                }));
            } else if rows.is_empty() {
                println!("no harness on this machine holds a grant; nothing to sync");
            } else {
                for row in &rows {
                    println!(
                        "{:<9} {:<7} {:<12} {:<40} {}",
                        row.result,
                        row.harness,
                        row.provider,
                        row.account.as_deref().unwrap_or("account not recorded"),
                        row.detail
                    );
                }
            }
            if failed > 0 {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
