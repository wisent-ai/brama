//! `brama subscriptions`: the subscription pool this deployment routes over,
//! and each provider's own usage report when it is asked for.

pub(crate) mod credentials;

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
}

// The operator's own console: this process holds the vault and the
// ledger, so the deployment scope is what it can prove.
pub(crate) async fn report(args: SubscriptionsArgs) {
    let SubscriptionsArgs {
        json,
        refresh_usage,
    } = args;
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
        if let Some(error) = text(row, "last_redeem_error") {
            println!("    last_redeem_error: {error}");
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
