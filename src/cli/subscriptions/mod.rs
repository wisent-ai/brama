//! `brama subscriptions`: the subscription pool this deployment routes over,
//! and each provider's own usage report when it is asked for.
//!
//! The pool an operator needs to read is usually not this process's: the
//! gateway runs on the fleet's host and the operator is at a workstation. So
//! the same report is readable from here against that gateway, resolved and
//! authenticated through Stado, and every member says who refreshes its
//! grant.

pub(crate) mod credentials;
mod manual;
pub(crate) mod remote;
pub(crate) mod unattended;
pub(crate) mod verdicts;

use clap::Args;
use serde_json::Value;

#[derive(Args)]
pub(crate) struct SubscriptionsArgs {
    /// Print the report as JSON instead of lines
    #[arg(long, default_value_t = false)]
    json: bool,
    /// Read current free provider usage reports; never starts a sign-in or model request
    #[arg(long, default_value_t = false)]
    refresh_usage: bool,
    /// Apply a pool membership document from stdin; uses the same bank/retire contract as HTTP
    #[arg(long, conflicts_with = "refresh_usage")]
    apply: bool,
    /// Read the pool of the gateway at this origin instead of this process's own
    #[arg(long, conflicts_with_all = ["apply", "gateway_consumer"])]
    gateway: Option<String>,
    /// Read the pool of the gateway Stado's service directory gives this consumer
    #[arg(long, conflicts_with = "apply")]
    gateway_consumer: Option<String>,
    /// Read the console's bearer from the vault as `<item>#<field>` instead of from stdin
    #[arg(long, conflicts_with = "apply")]
    bearer_item: Option<String>,
}

// The operator's own console: this process holds the vault and the
// ledger, so the deployment scope is what it can prove.
pub(crate) async fn report(args: SubscriptionsArgs) {
    let SubscriptionsArgs {
        json,
        refresh_usage,
        apply,
        gateway,
        gateway_consumer,
        bearer_item,
    } = args;
    if apply {
        let body = match std::io::read_to_string(std::io::stdin()) {
            Ok(body) => body,
            Err(error) => {
                eprintln!("cannot read subscription membership from stdin: {error}");
                std::process::exit(1);
            }
        };
        match brama::core::server::apply_subscription_membership(body.as_bytes()).await {
            Ok(receipt) => crate::cli::print_json(&receipt),
            Err(error) => {
                crate::cli::print_json(&error);
                std::process::exit(1);
            }
        }
        return;
    }
    let destination = remote::Destination {
        gateway,
        gateway_consumer,
        bearer_item,
    };
    if destination.gateway.is_some() || destination.gateway_consumer.is_some() {
        remote_report(destination, refresh_usage, json).await;
        return;
    }
    if destination.bearer_item.is_some() {
        eprintln!("--bearer-item is for a gateway; name --gateway or --gateway-consumer");
        std::process::exit(1);
    }
    let scope = brama::subscription_dispatch::pool::PoolScope::Deployment;
    // Two capabilities, one per question, and the same document from
    // either: the pool states what this deployment has recorded, plan
    // usage reads each provider's own usage report first.
    let report = if refresh_usage {
        brama::subscription_dispatch::plan_usage::report(&scope).await
    } else {
        brama::subscription_dispatch::pool::report(&scope).await
    };
    if json {
        crate::cli::print_json(&report);
    } else {
        print_pool(&report);
    }
    if report.get("ok").and_then(Value::as_bool) != Some(true) {
        std::process::exit(1);
    }
}

/// Read one gateway's own pool report and print it exactly as the local one
/// is printed.
///
/// Usage reports are the gateway's to fetch, so `--refresh-usage` is refused
/// here by name rather than answered with a report that did not do it.
async fn remote_report(destination: remote::Destination, refresh_usage: bool, json: bool) {
    if refresh_usage {
        eprintln!(
            "--refresh-usage reads providers from the gateway that holds the credentials; run it there, or read this gateway's recorded pool without it"
        );
        std::process::exit(1);
    }
    let mut stdin_bearer = zeroize::Zeroizing::new(String::new());
    if destination.bearer_item.is_none() {
        if let Err(error) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin_bearer)
        {
            eprintln!("reading the console bearer from stdin: {error}");
            std::process::exit(1);
        }
    }
    let (origin, bearer) = match destination.resolve(&stdin_bearer).await {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let Some(origin) = origin else {
        eprintln!("name --gateway or --gateway-consumer to read another gateway's pool");
        std::process::exit(1);
    };
    let report = match remote::pool_report(&origin, bearer.trim()).await {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    if json {
        crate::cli::print_json(&report);
    } else {
        println!("pool of the gateway at {origin}");
        print_pool(&report);
    }
    if report.get("ok").and_then(Value::as_bool) != Some(true) {
        std::process::exit(1);
    }
}

/// How many accounts the pool holds, which is not how many members it has,
/// and which accounts those are.
///
/// A member belongs to an account through the address its own sign-in named,
/// or failing that the Weles login item it signs in through; several members
/// can belong to one account, and a member that names neither is not an
/// account at all. Printing only the member count answered "fifteen" for a
/// deployment whose operator holds five accounts, and printing only the
/// number answered "three" without saying which three, which is not
/// checkable against the accounts the operator knows they hold.
fn print_accounts(accounts: Option<&Value>) {
    let Some(accounts) = accounts else { return };
    let per_provider = accounts
        .get("per_provider")
        .and_then(Value::as_object)
        .map(|providers| {
            providers
                .iter()
                .map(|(provider, held)| {
                    let named = held
                        .as_array()
                        .map(|held| {
                            held.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    format!(
                        "{provider} {}: {named}",
                        held.as_array().map_or(0, Vec::len)
                    )
                })
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    let total = accounts
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    println!(
        "{total} account(s){}",
        if per_provider.is_empty() {
            String::new()
        } else {
            format!(": {per_provider}")
        }
    );
    let count = |field: &str| {
        accounts
            .get(field)
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    };
    println!(
        "{} member(s) declare no account, {} remembered only by the usage ledger",
        count("members_without_account"),
        count("ledger_only_members")
    );
}

/// The same subscription report as the desktop, including partial failures.
fn print_pool(report: &Value) {
    let rows: &[Value] = report
        .get("subscriptions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let live = rows
        .iter()
        .filter(|row| text(row, "state") == Some("live"))
        .count();
    println!("{live} of {} subscription credentials are live", rows.len());
    print_accounts(report.get("accounts"));
    println!(
        "usage report: {}",
        if report.get("ok").and_then(Value::as_bool) == Some(true) {
            "complete"
        } else {
            "incomplete"
        }
    );

    for row in rows {
        println!(
            "{:<8} {:<14} {}{}",
            text(row, "state").unwrap_or("unknown"),
            text(row, "provider").unwrap_or("unknown"),
            text(row, "id").unwrap_or("unknown"),
            text(row, "label")
                .map(|label| format!(" ({label})"))
                .unwrap_or_default()
        );
        if let Some(expires_at) = text(row, "expires_at") {
            println!("    expires_at: {expires_at}");
        }
        // Who rotates this grant. A grant borrowed from a harness on the
        // operator's machine is that harness's to refresh: Brama rotating it
        // too revokes the refresh token the harness holds, and the operator
        // is signed out of their own session within the hour. Reading the
        // pool is the only place that answer is visible before it happens.
        match row
            .pointer("/credential/borrowed_from")
            .and_then(Value::as_str)
        {
            Some(harness) => println!(
                "    refreshed by: {harness}, the harness that holds it; Brama does not rotate it"
            ),
            None => println!("    refreshed by: brama"),
        }
        if let Some(error) = text(row, "last_redeem_error") {
            println!("    last_redeem_error: {error}");
        }
        // An account the gateway cannot sign in by itself is the state that
        // outlives every other line here: a grant expires and is replaced, but
        // a missing declaration stays until somebody reads this.
        if let Some(automatic) = row.get("automatic_sign_in").filter(|state| {
            state.get("applies").and_then(Value::as_bool) == Some(true)
                && state.get("automatic").and_then(Value::as_bool) == Some(false)
        }) {
            println!(
                "    sign_in_blocked: {}: {}",
                text(automatic, "blocked_by").unwrap_or("unknown"),
                text(automatic, "detail").unwrap_or("no reason was stated")
            );
        }
        if let Some(check) = row.get("usage_check").filter(|check| !check.is_null()) {
            println!(
                "    usage checked: {} ({})",
                instant(check.get("attempted_at_ms")),
                if check.get("ok").and_then(Value::as_bool) == Some(true) {
                    "succeeded"
                } else {
                    "failed"
                }
            );
            if let Some(detail) = text(check, "detail") {
                println!("    usage detail: {detail}");
            }
        }
        let limits = row
            .get("limits")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if limits.is_empty() {
            println!("    no usage measurement available");
        }
        for limit in limits {
            let amount = limit
                .get("used_fraction")
                .and_then(Value::as_f64)
                .map(|fraction| format!("{:.1}%", fraction * 100.0))
                .unwrap_or_else(|| "unknown".to_string());
            println!(
                "    {}: {amount} used; reset {}; observed {}{}",
                text(limit, "window_label")
                    .or_else(|| text(limit, "label"))
                    .unwrap_or("unnamed window"),
                instant(limit.get("resets_at_ms")),
                instant(limit.get("recorded_at_ms")),
                if row.get("stale").and_then(Value::as_bool) == Some(true) {
                    " (stale)"
                } else {
                    ""
                }
            );
        }
    }
    if let Some(errors) = report.get("errors").and_then(Value::as_array) {
        for error in errors {
            let account = error
                .pointer("/context/subscription")
                .and_then(Value::as_str)
                .or_else(|| error.pointer("/context/agent").and_then(Value::as_str))
                .unwrap_or("subscription report");
            let mut current = Some(error);
            let mut prefix = "";
            while let Some(failure) = current {
                eprintln!(
                    "{account}: {prefix}{}: {}",
                    text(failure, "failure_point").unwrap_or("unknown operation"),
                    text(failure, "detail").unwrap_or("failed without a stated reason")
                );
                current = failure.get("cause").filter(|cause| cause.is_object());
                prefix = "caused by ";
            }
        }
    }
}

fn instant(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|at| at.to_rfc3339())
        .unwrap_or_else(|| "unknown".to_string())
}

/// One string field, absent when the report states nothing there. A `null` reads
/// as absent, which is what the pool report writes for an expiry a credential
/// does not state and for a refusal there has not been.
fn text<'a>(report: &'a Value, key: &str) -> Option<&'a str> {
    report.get(key).and_then(Value::as_str)
}
