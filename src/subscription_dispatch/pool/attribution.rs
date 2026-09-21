//! Which provider account each pool member belongs to, and the count that
//! follows from it.
//!
//! A row is a member, not an account. Several members can carry one account
//! -- a grant taken from a harness and the same account signed in by the
//! gateway are two rows of one subscription -- and a member the vault no
//! longer lists is no account at all, so a row count answers a different
//! question from the one an operator asked.
//!
//! Nothing here is read from a name. A member's id, its label and the login
//! row it signs in through are names, and a name is not an account: one
//! Google login row can back both a Claude Code and a Codex subscription of
//! one person, so a count that falls back on the login row reports two
//! accounts as one and a login of one provider as an account of another.
//!
//! An account is the one thing the member itself records: the address on its
//! vault item, written from what the provider signed into its grant, read
//! from the item's `brama:account:` tag or the item's own `account_ref`.
//! Nothing else counts. A sign-in journal entry was tried here and removed:
//! it records the address one sign-in was *attempted* for, and an attempt
//! under a third address then counts as an account nobody holds.
//!
//! A member that records no account is reported as unattributed rather than
//! counted, and `brama subscription attribute <provider>` records it from
//! that member's own grant.

use std::collections::BTreeMap;

use serde_json::{json, Value};

/// The account one member records, or `None` when it records none.
fn identity(row: &Value) -> Option<&str> {
    row.get("account")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|account| !account.is_empty())
}

/// Which accounts need a second factor to be signed in, and which of them
/// hold the secret that answers one.
///
/// Asked how many of its accounts need two-factor authentication, this
/// product had no answer: the vault records whether a seed is stored, the
/// provider's requirement is learned only by trying to sign in, and neither
/// was reported beside the other. An account with no seed and no attempt is
/// not an account without a second factor, so this states `unknown` for it
/// rather than counting it either way.
///
/// One vault pass answers the seed for every login, and the requirement
/// comes from the sign-ins this gateway already ran, so nothing is asked of
/// a provider and no browser starts.
pub async fn second_factor_report(provider: Option<&str>) -> Result<Value, String> {
    let provider = provider.map(str::trim).filter(|named| !named.is_empty());
    let seeds = crate::gateway::broker::login_seed_states();
    let mut accounts: BTreeMap<String, Value> = BTreeMap::new();
    for entry in crate::gateway::broker::list_all_subscriptions().await? {
        if provider.is_some_and(|named| entry.provider != named) || entry.status != "active" {
            continue;
        }
        let row = super::account::subscription_view(&entry);
        let account = identity(&row)
            .map(str::to_owned)
            .unwrap_or_else(|| entry.id.clone());
        let login = row
            .pointer("/second_factor/login_item")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let seed = login
            .as_deref()
            .and_then(|login| seeds.get(login).cloned())
            .unwrap_or_else(|| "no_login_declared".to_owned());
        accounts.insert(
            format!("{}\u{1f}{account}", entry.provider),
            json!({
                "provider": entry.provider,
                "account": account,
                "member": entry.id,
                "login_item": login,
                "required": row.pointer("/second_factor/required").cloned(),
                "evidence": row.pointer("/second_factor/evidence").cloned(),
                "seed": seed,
            }),
        );
    }
    let counted = |wanted: Option<bool>| {
        accounts
            .values()
            .filter(|row| row.get("required").and_then(Value::as_bool) == wanted)
            .count()
    };
    Ok(json!({
        "provider": provider,
        "required": counted(Some(true)),
        "not_required": counted(Some(false)),
        "unknown": counted(None),
        "accounts": accounts.into_values().collect::<Vec<_>>(),
    }))
}

/// Record the account every member of one provider belongs to, from what
/// each member's own grant states.
///
/// The pool can only count accounts that were recorded, and a member
/// imported before the account was recorded beside its grant records none,
/// so such members are reported as unattributed however exactly their own
/// ids happen to name an account. This is the command that closes that gap,
/// and the sweep runs the same code on its own pass, so it closes by itself
/// for every member imported later.
///
/// Nothing is rotated and nothing is asked of a provider: each grant is
/// opened, read for the address its issuer signed into it, and written back
/// unchanged beside the account it names. A member whose grant states no
/// address stays unattributed and is reported with the reason.
pub async fn record_accounts(provider: &str) -> Result<Value, String> {
    let provider = provider.trim();
    if provider.is_empty() {
        return Err("a provider is required".into());
    }
    let members: Vec<String> = crate::gateway::broker::list_all_subscriptions()
        .await?
        .into_iter()
        .filter(|entry| entry.provider == provider && entry.status == "active")
        .map(|entry| entry.id)
        .collect();
    if members.is_empty() {
        return Err(format!(
            "this deployment's pool holds no `{provider}` member, so there is no grant to read \
             an account from"
        ));
    }
    let mut recorded = Vec::new();
    let mut unattributed = Vec::new();
    for id in members {
        match crate::gateway::broker::record_subscription_account(&id, provider).await {
            Ok(Some(account)) => recorded.push(json!({"member": id, "account": account})),
            Ok(None) => unattributed.push(json!({
                "member": id,
                "reason": "this member's grant states no account, so nothing was recorded",
            })),
            Err(detail) => unattributed.push(json!({"member": id, "reason": detail})),
        }
    }
    Ok(json!({
        "provider": provider,
        "recorded": recorded,
        "unattributed": unattributed,
    }))
}

/// The pool's accounts: how many this deployment holds, which accounts they
/// are per provider, and what could not be attributed to one.
///
/// The accounts are stated, not just counted, because the number is only
/// checkable against the accounts an operator knows they hold if the
/// document says which ones it means.
pub(super) fn accounts(rows: &[Value]) -> Value {
    let mut per_provider: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut unattributed: Vec<String> = Vec::new();
    let mut ledger_only: Vec<String> = Vec::new();
    for row in rows {
        let id = row.get("id").and_then(Value::as_str).unwrap_or_default();
        let provider = row
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if row.get("status").and_then(Value::as_str) != Some("active") {
            ledger_only.push(id.to_string());
            continue;
        }
        match identity(row) {
            Some(account) => {
                // One address is one account however it is capitalised.
                let holders = per_provider.entry(provider.to_string()).or_default();
                if !holders
                    .iter()
                    .any(|held| held.eq_ignore_ascii_case(account))
                {
                    holders.push(account.to_string());
                }
            }
            None => unattributed.push(id.to_string()),
        }
    }
    let total: usize = per_provider.values().map(Vec::len).sum();
    json!({
        "total": total,
        "per_provider": per_provider,
        "members_without_account": unattributed,
        "ledger_only_members": ledger_only,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, provider: &str, recorded: Option<&str>, attempted: Option<&str>) -> Value {
        json!({
            "id": id,
            "provider": provider,
            "status": "active",
            "login_item": "a-login-row",
            "account": recorded,
            "sign_in": attempted.map(|account| json!({ "account": account })),
        })
    }

    /// The account its vault item records is the account; the login row it
    /// signs in through is not consulted at all.
    #[test]
    fn a_recorded_account_is_the_account() {
        let held = accounts(&[member(
            "held-claude-code",
            "claude-code",
            Some("holder@example.invalid"),
            None,
        )]);
        assert_eq!(held["total"], 1);
        assert_eq!(
            held["per_provider"]["claude-code"][0],
            "holder@example.invalid"
        );
        assert!(held["members_without_account"]
            .as_array()
            .is_some_and(Vec::is_empty));
    }

    /// An attempt is not an account: a member that records none is reported,
    /// however many sign-ins were tried against it and whichever address
    /// each of them named. Counting the journal made a deployment holding
    /// five accounts answer six.
    #[test]
    fn an_attempted_address_is_not_an_account() {
        let held = accounts(&[member(
            "codex-primary",
            "codex",
            None,
            Some("tried@example.invalid"),
        )]);
        assert_eq!(held["total"], 0);
        assert_eq!(held["members_without_account"][0], "codex-primary");
    }

    /// Two rows of one account are one account: the grant a harness holds and
    /// the same address signed in by the gateway are the same subscription.
    #[test]
    fn two_members_of_one_address_are_one_account() {
        let held = accounts(&[
            member("held", "codex", Some("holder@example.invalid"), None),
            member("signed-in", "codex", Some("Holder@example.invalid"), None),
        ]);
        assert_eq!(held["total"], 1);
    }

    /// One address holding an account with two providers is two accounts: a
    /// Claude Code subscription and a Codex one are bought separately.
    #[test]
    fn one_address_at_two_providers_is_two_accounts() {
        let held = accounts(&[
            member(
                "claude",
                "claude-code",
                Some("holder@example.invalid"),
                None,
            ),
            member("codex", "codex", Some("holder@example.invalid"), None),
        ]);
        assert_eq!(held["total"], 2);
        assert_eq!(
            held["per_provider"]["claude-code"][0],
            "holder@example.invalid"
        );
        assert_eq!(held["per_provider"]["codex"][0], "holder@example.invalid");
    }

    /// A member that records no account is reported and not counted, however
    /// exactly its own id or its login row happens to name one; a member the
    /// vault no longer lists is neither.
    #[test]
    fn what_is_not_recorded_is_reported_instead_of_counted() {
        let held = accounts(&[
            json!({
                "id": "brama-sub-held-codex-a-holder",
                "provider": "codex",
                "status": "active",
                "login_item": "a-shared-google-login",
                "account": null,
                "sign_in": null,
            }),
            json!({ "id": "forgotten", "provider": "codex", "status": "undiscovered" }),
        ]);
        assert_eq!(held["total"], 0);
        assert_eq!(
            held["members_without_account"][0],
            "brama-sub-held-codex-a-holder"
        );
        assert_eq!(held["ledger_only_members"][0], "forgotten");
    }
}
