//! Who is in the pool: recording which account a member belongs to, putting
//! a retired member back, and giving one away.
//!
//! These three act on membership rather than on a credential, which is why
//! they are not beside the sign-ins: none of them obtains, rotates or reads a
//! grant. They travel together because they answer one question -- which
//! accounts does this deployment use -- and because a retirement that
//! nothing could take back leaves declared accounts unusable while the pool
//! reports itself empty.

use std::future::Future;

use serde_json::Value;

use super::remote::Destination;
use super::text;

/// Record which account each member of one provider belongs to, read from
/// that member's own grant, and exit non-zero while any member is left
/// unattributed: the pool's account count is short by exactly those.
///
/// Named a gateway, the gateway records them in the vault it serves from,
/// which is the vault Weles resolves a sign-in from. Named none, this host's
/// own vault program records them: on a workstation that reads the fleet
/// vault remotely that is a local copy Weles never reads, so the account
/// reaches no sign-in.
pub(super) async fn attribute(destination: Destination, provider: &str, json: bool) {
    let verdict = match destination.resolve_reading_stdin().await {
        Ok((Some(gateway), bearer)) => {
            super::remote::attribute(&gateway, bearer.trim(), provider).await
        }
        Ok((None, _)) => brama::subscription_dispatch::pool::record_accounts(provider).await,
        Err(error) => Err(error),
    };
    match verdict {
        Ok(verdict) => {
            if json {
                crate::cli::print_json(&verdict);
            } else {
                print_attribution(&verdict);
            }
            if verdict
                .get("unattributed")
                .and_then(Value::as_array)
                .is_some_and(|left| !left.is_empty())
            {
                crate::cli::fail();
            }
        }
        Err(error) => crate::cli::refused(error),
    }
}

/// Report which accounts need a second factor, and which hold the secret
/// that answers one.
///
/// Named a gateway, the gateway reads the vault it serves from, which holds
/// the seeds Weles answers a second factor with. Named none, this host's own
/// vault program reads them, and on a workstation that reads the fleet vault
/// remotely that program refuses and names the vault owner.
pub(super) async fn second_factor(destination: Destination, provider: Option<&str>, json: bool) {
    let report = match destination.resolve_reading_stdin().await {
        Ok((Some(gateway), bearer)) => {
            super::remote::second_factor(&gateway, bearer.trim(), provider).await
        }
        Ok((None, _)) => brama::subscription_dispatch::pool::second_factor_report(provider)
            .await
            .map_err(|error| {
                format!(
                    "{error}\nthe gateway that serves the pool reads the vault Weles signs in \
                     from: brama subscription authenticator list --gateway-consumer <consumer> \
                     --bearer-role <role>"
                )
            }),
        Err(error) => Err(error),
    };
    match report {
        Ok(report) => {
            if json {
                crate::cli::print_json(&report);
                return;
            }
            // A count or field the gateway left out is named as such, never
            // read as zero accounts or an empty name.
            let count = |field: &str| match report.get(field).and_then(Value::as_u64) {
                Some(count) => count.to_string(),
                None => format!("an unreported number ({field} absent)"),
            };
            let field = |row: &Value, name: &str| match text(row, name) {
                Some(value) => value.to_string(),
                None => format!("<{name} not reported>"),
            };
            println!(
                "{} account(s) asked for a second factor, {} did not ask in the observed sign-in, {} unobserved",
                count("required"),
                count("not_required"),
                count("unknown")
            );
            let Some(accounts) = report.get("accounts").and_then(Value::as_array) else {
                println!("  the report lists no accounts array");
                return;
            };
            for row in accounts {
                let needs = match row.get("required").and_then(Value::as_bool) {
                    Some(true) => "needs a second factor",
                    Some(false) => "not requested in the observed sign-in",
                    None => "unobserved",
                };
                println!(
                    "  {} {} -> {needs}; seed {}",
                    field(row, "provider"),
                    field(row, "account"),
                    field(row, "seed")
                );
                if let Some(evidence) = text(row, "evidence") {
                    println!("    {evidence}");
                }
            }
        }
        Err(error) => crate::cli::refused(error),
    }
}

/// Put one retired member back in the rotation.
///
/// Named a gateway, this asks that gateway; named none, it acts on this
/// host's own pool through the same code the gateway's route runs, because a
/// deployment whose pool is on this machine has no remote to ask.
pub(super) async fn reinstate(
    destination: Destination,
    subscription_id: &str,
    reason: &str,
    json: bool,
) {
    if destination.gateway.is_none() && destination.gateway_consumer.is_none() {
        answered(
            subscription_id,
            brama::subscription_dispatch::pool::reinstate_member(subscription_id, reason).await,
            json,
        );
        return;
    }
    act(destination, subscription_id, json, |gateway, bearer| {
        let subscription_id = subscription_id.to_owned();
        let reason = reason.to_owned();
        async move { super::remote::reinstate(&gateway, &bearer, &subscription_id, &reason).await }
    })
    .await;
}

/// One membership change made on this host: its verdict said, or its
/// refusal printed as it is and the command ended unsuccessfully. The member
/// is the one asked about; a verdict without a detail is printed whole rather
/// than as an empty sentence.
fn answered<Refused>(subscription_id: &str, outcome: Result<Value, (Refused, String)>, json: bool) {
    match outcome {
        Ok(verdict) => match text(&verdict, "detail") {
            Some(detail) => said(subscription_id, detail, json),
            None => said(subscription_id, &verdict.to_string(), json),
        },
        Err((_, error)) => crate::cli::refused(error),
    }
}

/// Drop one member the pool only remembers.
///
/// Named a gateway, this asks that gateway; named none, it acts on this
/// host's own usage history through the same code the gateway's route runs.
pub(super) async fn forget(
    destination: Destination,
    subscription_id: &str,
    reason: &str,
    json: bool,
) {
    if destination.gateway.is_none() && destination.gateway_consumer.is_none() {
        answered(
            subscription_id,
            brama::subscription_dispatch::forget::forget_member(subscription_id, reason).await,
            json,
        );
        return;
    }
    act(destination, subscription_id, json, |gateway, bearer| {
        let subscription_id = subscription_id.to_owned();
        let reason = reason.to_owned();
        async move { super::remote::forget(&gateway, &bearer, &subscription_id, &reason).await }
    })
    .await;
}

/// Give one member back on the gateway that holds it.
pub(super) async fn disown(
    destination: Destination,
    subscription_id: &str,
    reason: &str,
    json: bool,
) {
    act(destination, subscription_id, json, |gateway, bearer| {
        let subscription_id = subscription_id.to_owned();
        let reason = reason.to_owned();
        async move { super::remote::disown(&gateway, &bearer, &subscription_id, &reason).await }
    })
    .await;
}

/// Resolve the gateway and its bearer once, then run one membership call
/// against it. Both calls refuse identically when no gateway is named,
/// because the member and its journal live on the gateway, not in a shell.
async fn act<Call, Running>(destination: Destination, subscription_id: &str, json: bool, call: Call)
where
    Call: FnOnce(String, String) -> Running,
    Running: Future<Output = Result<String, String>>,
{
    match destination.resolve_reading_stdin().await {
        Ok((Some(gateway), bearer)) => match call(gateway, bearer.trim().to_owned()).await {
            Ok(detail) if json => said(subscription_id, &detail, true),
            Ok(detail) => println!("{detail}"),
            Err(error) => crate::cli::refused(error),
        },
        Ok((None, _)) => crate::cli::refused("name the gateway holding the member: --gateway or --gateway-consumer"),
        Err(error) => crate::cli::refused(error),
    }
}

/// One membership change: `<member>: <detail>`, or `{subscription_id, detail}`
/// with `--json`.
fn said(subscription_id: &str, detail: &str, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({ "subscription_id": subscription_id, "detail": detail })
        );
    } else {
        println!("{subscription_id}: {detail}");
    }
}

/// What the attribution recorded, and what it could not.
///
/// The members it could not attribute are printed with the reason, because
/// the pool's account count is short by exactly them, and an operator
/// comparing that count with the accounts they hold needs to see which
/// member is missing rather than a number that disagrees.
fn print_attribution(verdict: &Value) {
    let rows = |field: &str| -> Option<Vec<&Value>> {
        verdict
            .get(field)
            .and_then(Value::as_array)
            .map(|rows| rows.iter().collect())
    };
    let (Some(recorded), Some(unattributed)) = (rows("recorded"), rows("unattributed")) else {
        crate::cli::refused(format!(
            "the attribution verdict does not list recorded and unattributed members: {verdict}"
        ));
    };
    println!(
        "{} of {} member(s) name an account",
        recorded.len(),
        recorded.len().saturating_add(unattributed.len())
    );
    let field = |row: &Value, name: &str| match text(row, name) {
        Some(value) => value.to_string(),
        None => format!("<{name} not reported>"),
    };
    for row in recorded {
        println!("  {} -> {}", field(row, "member"), field(row, "account"));
    }
    for row in unattributed {
        println!("  {}: {}", field(row, "member"), field(row, "reason"));
    }
}
