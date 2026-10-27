//! The lines of `brama subscription list` that say where a member stands —
//! discovered, signed in, or retired with the cause the gateway recorded —
//! and what the last purchase of each declared provider did. Brama Desktop
//! shows the same fields in its Membership section and purchase panel.

use serde_json::Value;

use super::{instant, text};

/// Whether the gateway reports `row` retired, by its flag or its status.
pub(super) fn is_retired(row: &Value) -> bool {
    row.get("retired").and_then(Value::as_bool) == Some(true)
        || text(row, "status") == Some("retired")
}

/// One member's standing, after its identity line.
pub(super) fn print_membership(row: &Value) {
    let credential = row.get("credential").filter(|value| value.is_object());
    if let Some(cause) = credential.and_then(|credential| text(credential, "journal_error")) {
        println!("    membership: unknown; the retirement journal could not be read: {cause}");
        return;
    }
    if is_retired(row) {
        match credential.and_then(|credential| text(credential, "cause")) {
            Some(cause) => println!(
                "    retired: {cause} (recorded {})",
                instant(credential.and_then(|credential| credential.get("recorded_at_ms")))
            ),
            None => println!("    retired: the gateway recorded this retirement without a cause"),
        }
        return;
    }
    match row.get("sign_in").filter(|value| value.is_object()) {
        Some(sign_in) => println!(
            "    last sign-in: {} at {}: {}",
            sign_in["result"],
            instant(sign_in.get("at_ms")),
            sign_in["detail"]
        ),
        None => match row.get("discovery").filter(|value| value.is_object()) {
            Some(discovery) => println!(
                "    membership: discovered account {} has not been signed in by Brama",
                discovery["account"]
            ),
            None => println!("    membership: no sign-in or discovery is recorded"),
        },
    }
}

/// The newest journaled purchase per declared provider, from the pool
/// document's `acquisition` object; a gateway without the object says so.
pub(super) fn print_acquisitions(report: &Value) {
    let Some(acquisition) = report.get("acquisition") else {
        println!("last purchase: not reported by this gateway");
        return;
    };
    let Some(providers) = acquisition.as_object() else {
        println!("last purchase: the acquisition journal could not be read (see errors)");
        return;
    };
    for (provider, record) in providers {
        if !record.is_object() {
            println!("last purchase of {provider}: none journaled");
            continue;
        }
        println!(
            "last purchase of {provider}: {} at {}: {}",
            record["result"],
            instant(record.get("at_ms")),
            record["detail"]
        );
        let context: Vec<String> = [
            ("asked by", "trigger"),
            ("reason", "reason"),
            ("shortage", "shortage"),
            ("account", "account"),
            ("plan", "plan_tier"),
            ("request", "request_id"),
        ]
        .into_iter()
        .filter_map(|(label, key)| text(record, key).map(|value| format!("{label}: {value}")))
        .collect();
        if !context.is_empty() {
            println!("    {}", context.join(" · "));
        }
    }
}

/// The ids the usage ledger remembers that are no pool member, from the pool
/// document's `remembered` list: counted nowhere, each dropped with `brama
/// subscription forget`. A gateway without the list says nothing of them.
pub(super) fn print_remembered(report: &Value) {
    let remembered: Vec<&Value> = report
        .get("remembered")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    if remembered.is_empty() {
        return;
    }
    println!(
        "remembered by the usage history only, not pool members ({}): drop each with brama subscription forget --subscription-id <id> --reason <why>",
        remembered.len()
    );
    for entry in remembered {
        println!(
            "    {} ({})",
            text(entry, "id").into_iter().collect::<String>(),
            text(entry, "provider").into_iter().collect::<String>()
        );
    }
}
