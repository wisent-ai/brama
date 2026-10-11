//! Printing what each account has left, one account per line and one line per
//! plan window, the way an operator deciding where a session goes reads it.

use serde_json::Value;

use crate::subscription_dispatch::pool::AccountCapacity;

use super::instant;

/// One line per account and one per window of the document's
/// `accounts.capacity`: whether a session put on the account would be served,
/// how much of each window is left and when it resets, the sessions the
/// account carries, and every member the ledger refuses with its reason.
///
/// A document without `accounts.capacity` comes from a gateway older than
/// the capacity reading and says so; one whose capacity does not read as the
/// shape this CLI knows is named with the decoding cause, because a shape
/// drift between the gateway and this program is a finding, not an empty
/// pool.
pub(super) fn print_capacity(accounts: &Value) {
    let Some(capacity) = accounts.get("capacity") else {
        println!(
            "account capacity: not stated by this gateway (older than the capacity reading)"
        );
        return;
    };
    let capacity: Vec<AccountCapacity> = match serde_json::from_value(capacity.clone()) {
        Ok(capacity) => capacity,
        Err(cause) => {
            eprintln!(
                "account capacity: the gateway's accounts.capacity could not be read: {cause}"
            );
            return;
        }
    };
    for account in &capacity {
        let mut line = format!(
            "{} {}: {}",
            account.provider,
            account.account,
            if account.available {
                "available"
            } else {
                "not available"
            }
        );
        match account.live_sessions {
            Some(sessions) => line.push_str(&format!("; live sessions: {sessions}")),
            None => line.push_str("; live sessions: unknown (lease register unread)"),
        }
        if let Some(until) = account.blocked_until_ms {
            line.push_str(&format!(
                "; blocked until {}",
                instant(Some(&Value::from(until)))
            ));
        }
        println!("{line}");
        if account.windows.is_empty() {
            println!("    no plan window has been read for this account");
        }
        for window in &account.windows {
            let left = match window.remaining_fraction {
                Some(fraction) => format!("{fraction:.2} of the window left"),
                None => "rolled, not read since".to_string(),
            };
            // The provider's window label where it gave one, else the
            // limit's own label: both are the provider's words.
            let named = match &window.window_label {
                Some(window_label) => window_label,
                None => &window.label,
            };
            println!(
                "    {named}: {left}; reset {}; read {} off {}",
                instant(window.resets_at_ms.map(Value::from).as_ref()),
                instant(Some(&Value::from(window.read_at_ms))),
                window.member
            );
        }
        for refused in &account.refused_members {
            println!("    refused member {}: {}", refused.member, refused.refusal);
        }
    }
}
