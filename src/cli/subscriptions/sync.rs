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

pub(crate) mod borrowing;
pub(crate) mod unattended;

use std::io::Read;

use serde::Serialize;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use brama::subscription_dispatch::sign_in::manual::{self, HeldGrant, ManualSignIn};

use super::harness;

/// Reading the pool report waits a minute; the gateway answers 2xx on success.
const POOL_READ_TIMEOUT_SECONDS: u64 = 60;
const HTTP_SUCCESS: std::ops::Range<u16> = 200..300;

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

/// Each member the pool refreshes itself, with the expiry it recorded for
/// its grant, read where the sweep will write: through the gateway with the
/// console's bearer, or this process's own pool report.
///
/// A borrowed member is not here: the sweep hands those the harness's
/// current grant on every pass, and Brama never rotates them. A member the
/// pool has disowned - `burnt`, or a refresh it rejected - is not here
/// either, because that grant is dead and the harness's is what repairs it.
///
/// What is left is the dangerous kind, and the expiry is why it is read: a
/// member Brama refreshes itself while a harness on this machine holds the
/// same account. The provider rotates the refresh token on whichever of the
/// two refreshes first and revokes the other. Handing the account back to
/// the harness is what ends that, so the sweep does it as soon as the
/// harness's grant is the newer one - which it is the moment the operator
/// signs in again.
async fn self_refreshed(
    gateway: Option<&str>,
    bearer: &str,
) -> Result<Vec<(String, Option<i64>)>, String> {
    let report = match gateway {
        Some(gateway) => pool_report(gateway, bearer).await?,
        None => {
            brama::subscription_dispatch::pool::report(
                &brama::subscription_dispatch::pool::PoolScope::Deployment,
            )
            .await
        }
    };
    Ok(report["subscriptions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| {
            row["state"].as_str() != Some("burnt")
                && row["credential"]["state"].as_str() != Some("needs_reauthorization")
                && row["credential"]["borrowed_from"].is_null()
        })
        .filter_map(|row| {
            row["id"]
                .as_str()
                .map(|id| (id.to_owned(), row["credential"]["expires_at_ms"].as_i64()))
        })
        .collect())
}

/// The pool report of the gateway that actually serves, read with the
/// console's bearer.
///
/// The sweep reads it to decide what to hand over; `brama subscriptions`
/// reads the same document so an operator can see the same pool from the
/// machine the grants come from. One reader, so the two answers cannot
/// drift apart.
pub(crate) async fn pool_report(gateway: &str, bearer: &str) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(POOL_READ_TIMEOUT_SECONDS))
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
    if !HTTP_SUCCESS.contains(&status) {
        let message = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        return Err(format!(
            "the gateway refused the pool report with HTTP {status}: {message}"
        ));
    }
    Ok(body)
}

/// Where the sweep sends its grants and what it authenticates with.
///
/// A named gateway with the bearer on stdin is the console's arrangement. A
/// consumer name with a vault coordinate is the service's: both are resolved
/// through Stado at every pass, so the sweep survives a rotated token and a
/// moved port without anybody editing its declaration.
#[derive(Clone, Default)]
pub(crate) struct Destination {
    pub gateway: Option<String>,
    pub gateway_consumer: Option<String>,
    pub bearer_item: Option<String>,
    /// The operator saying, in as many words, that this machine may lose the
    /// session it is signed into. Off by default; see [`borrowing`].
    pub allow_cross_host: bool,
}

impl Destination {
    /// The gateway origin and console bearer for one pass, or nothing when
    /// the caller means this process's own pool.
    pub(crate) async fn resolve(
        &self,
        stdin_bearer: &Zeroizing<String>,
    ) -> Result<(Option<String>, Zeroizing<String>), String> {
        let gateway = match (&self.gateway, &self.gateway_consumer) {
            (Some(_), Some(_)) => {
                return Err("name --gateway or --gateway-consumer, not both".into())
            }
            (Some(gateway), None) => Some(gateway.clone()),
            (None, Some(consumer)) => Some(unattended::gateway_for_consumer(consumer).await?),
            (None, None) => None,
        };
        if gateway.is_none() {
            if self.bearer_item.is_some() {
                return Err("--bearer-item is for a gateway; name one".into());
            }
            return Ok((None, Zeroizing::new(String::new())));
        }
        if let Some(origin) = gateway.as_deref() {
            // The rule that keeps this machine's own sign-ins alive.
            if let Some(refusal) = borrowing::refuse_cross_host(origin, self.allow_cross_host) {
                return Err(refusal);
            }
        }
        let bearer = match &self.bearer_item {
            Some(coordinate) => unattended::bearer_from_vault(coordinate).await?,
            None => stdin_bearer.clone(),
        };
        if bearer.trim().is_empty() {
            return Err(
                "a gateway needs the console's bearer: --bearer-item, or the token on stdin".into(),
            );
        }
        Ok((gateway, bearer))
    }
}

/// Sweep every held grant into the pool, once.
pub(crate) async fn sync(
    reason: &str,
    home: Option<&str>,
    destination: &Destination,
) -> Result<Vec<SyncRow>, String> {
    if reason.trim().is_empty() {
        return Err("--reason must say why this sync is being run".into());
    }
    let mut stdin_bearer = Zeroizing::new(String::new());
    if destination.gateway.is_some() && destination.bearer_item.is_none() {
        std::io::stdin()
            .read_to_string(&mut stdin_bearer)
            .map_err(|error| format!("reading the console bearer from stdin: {error}"))?;
    }
    let (gateway, bearer) = destination.resolve(&stdin_bearer).await?;
    let gateway = gateway.as_deref();
    let bearer = bearer.trim();
    let held = manual::held(&harness::home(home), None)?;
    let pooled = self_refreshed(gateway, bearer).await?;
    let mut rows = Vec::with_capacity(held.len());
    for grant in held {
        rows.push(sweep_one(grant, &pooled, reason, gateway, bearer).await);
    }
    Ok(rows)
}

async fn sweep_one(
    grant: HeldGrant,
    pooled: &[(String, Option<i64>)],
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
    // A member Brama refreshes itself while this harness holds the same
    // account is the shape that signs the operator out: the provider
    // rotates the refresh token, Brama keeps the new one, and the copy the
    // harness still holds is revoked within the hour. Handing the account
    // back to the harness ends it, and the harness's grant is the one to
    // hand over as soon as it is the newer of the two - a grant the pool
    // rotated last is the live one, and the harness's is the dead copy
    // until the operator signs in again.
    if let Some((_, pooled_expiry)) = pooled.iter().find(|(id, _)| id == &subscription_id) {
        let harness_holds_the_newer = match (grant.expires_at_ms, pooled_expiry) {
            (Some(held), Some(pooled)) => held > *pooled,
            _ => true,
        };
        if !harness_holds_the_newer {
            return SyncRow {
                harness: harness_name.clone(),
                provider,
                account: Some(account),
                subscription_id: Some(subscription_id),
                result: "present",
                detail: format!(
                    "Brama holds a newer grant for this account and refreshes it itself, while \
                     {harness_name} holds an older copy of the same account: whichever of the two \
                     refreshes first revokes the other, and signing {harness_name} in again hands \
                     the account back to it"
                ),
            };
        }
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
        Ok(verdict) if verdict.result == "unchanged" => SyncRow {
            harness: harness_name,
            provider,
            account: Some(account),
            subscription_id: Some(subscription_id),
            result: "present",
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
            report(&rows, json);
            if rows.iter().any(|row| row.result == "failed") {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

/// Sweep on a cadence until the service is stopped.
///
/// A pass that fails is reported and the next one still runs: the reasons a
/// pass fails here — the gateway is restarting, the vault is briefly
/// unreadable, one provider refused a grant — are all conditions the next
/// pass can find repaired, and a sweep that exited on the first of them
/// would leave the pool exactly as empty as having no sweep at all.
pub(crate) async fn sync_every(
    seconds: u64,
    reason: &str,
    home: Option<&str>,
    destination: &Destination,
    json: bool,
) -> ! {
    let cadence = std::time::Duration::from_secs(seconds.max(1));
    loop {
        match sync(reason, home, destination).await {
            Ok(rows) => report(&rows, json),
            Err(error) => eprintln!("[subscription sync] pass failed: {error}"),
        }
        tokio::time::sleep(cadence).await;
    }
}

/// One sweep's rows, as the operator reads them.
fn report(rows: &[SyncRow], json: bool) {
    let failed = rows.iter().filter(|row| row.result == "failed").count();
    if json {
        crate::cli::print_json(&json!({
            "ok": failed == 0,
            "grants": rows,
        }));
        return;
    }
    if rows.is_empty() {
        println!("no harness on this machine holds a grant; nothing to sync");
        return;
    }
    for row in rows {
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
