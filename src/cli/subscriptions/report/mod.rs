//! Printing one pool report the way an operator reads it: the accounts behind
//! it, the subscriptions in it, and the instants and strings a report carries.
//!
//! The same lines are printed for a local gateway and for a remote one, which
//! is why the printing lives here rather than inside either caller.

use serde_json::Value;


/// How many accounts the pool holds, which is not how many members it has,
/// and which accounts those are.
///
/// A member belongs to an account through the address recorded against it;
/// several members can belong to one account, and a member that records
/// none is not an account at all. Printing only the member count answers
/// with the number of rows, which is a different number, and printing only
/// a total answers a count nobody can check against the accounts they hold.
pub(super) fn print_accounts(accounts: Option<&Value>) {
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
pub(super) fn print_pool(report: &Value) {
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
pub(super) fn text<'a>(report: &'a Value, key: &str) -> Option<&'a str> {
    report.get(key).and_then(Value::as_str)
}
