//! Real account capacity: the pool document's `accounts.capacity`, read from
//! the serving gateway after every provider usage report was brought current,
//! is the per-account projection of exactly the member rows it lists, and
//! `brama subscription list` prints it. Read-only: nothing is leased, bought
//! or refreshed beyond the usage report, and no provider is asked for
//! inference.
use super::Run;
use brama::subscription_dispatch::acquire::{at_limit, WHOLE_LIMIT};
use brama::subscription_dispatch::pool::AccountCapacity;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The active, non-retired rows that record an account, keyed the way the
/// capacity groups them: provider and lowercased address.
fn attributed_rows(pool: &Value) -> BTreeMap<(String, String), Vec<&Value>> {
    let mut rows: BTreeMap<(String, String), Vec<&Value>> = BTreeMap::new();
    for row in pool["subscriptions"].as_array().expect("pool members") {
        if row["status"] != "active" || row["retired"] == json!(true) {
            continue;
        }
        let Some(account) = row["account"].as_str().map(str::trim) else {
            continue;
        };
        if account.is_empty() {
            continue;
        }
        let provider = row["provider"]
            .as_str()
            .expect("member provider")
            .to_owned();
        rows.entry((provider, account.to_lowercase()))
            .or_default()
            .push(row);
    }
    rows
}

#[test]
#[ignore = "Requires a real gateway whose pool records accounts and whose providers publish usage reports"]
fn account_capacity_is_the_newest_reading_across_the_members_of_each_account() {
    let mut run = Run::new();
    let pool = run.pool(true);
    let observed_at_ms = pool["observed_at_ms"]
        .as_i64()
        .expect("the document states when it was observed");
    let capacity: Vec<AccountCapacity> =
        serde_json::from_value(pool["accounts"]["capacity"].clone())
            .expect("accounts.capacity reads as the capacity shape the CLI and Desktop decode");
    let rows = attributed_rows(&pool);
    assert_eq!(
        capacity.len(),
        rows.len(),
        "one capacity entry per attributed account: {:?} against {:?}",
        capacity
            .iter()
            .map(|account| (&account.provider, &account.account))
            .collect::<Vec<_>>(),
        rows.keys().collect::<Vec<_>>()
    );
    assert!(
        !capacity.is_empty(),
        "the qualification pool must attribute at least one account"
    );
    let mut evidence = Vec::new();
    for account in &capacity {
        let key = (account.provider.clone(), account.account.to_lowercase());
        let Some(members) = rows.get(&key) else {
            panic!("capacity names an account the rows do not attribute: {key:?}");
        };
        let mut member_ids: Vec<&str> = members
            .iter()
            .map(|row| row["id"].as_str().expect("member id"))
            .collect();
        member_ids.sort_unstable();
        let mut listed: Vec<&str> = account.members.iter().map(String::as_str).collect();
        listed.sort_unstable();
        assert_eq!(
            listed, member_ids,
            "{key:?}: members are the attributed rows"
        );
        for id in account.usable_members.iter() {
            assert!(
                account.members.contains(id),
                "{key:?}: usable member {id} is a member"
            );
        }
        for refused in &account.refused_members {
            assert!(
                account.members.contains(&refused.member),
                "{key:?}: refused member {} is a member",
                refused.member
            );
            assert!(
                !refused.refusal.trim().is_empty(),
                "{key:?}: a refusal states its reason"
            );
        }
        assert_eq!(
            account.usable_members.len() + account.refused_members.len(),
            account.members.len(),
            "{key:?}: every member is usable or refused"
        );
        // The live sessions are the rows' own counts, unknown as soon as one is.
        let carried = members
            .iter()
            .map(|row| row["live_sessions"].as_u64())
            .sum::<Option<u64>>();
        assert_eq!(
            account.live_sessions, carried,
            "{key:?}: live sessions are the rows' sum"
        );
        // Each window is the newest reading of that limit any member holds.
        for window in &account.windows {
            let Some(newest) = members
                .iter()
                .flat_map(|row| row["limits"].as_array().expect("member limits"))
                .filter(|limit| limit["limit_id"] == json!(window.limit_id))
                .max_by_key(|limit| limit["recorded_at_ms"].as_i64().expect("reading instant"))
            else {
                panic!("{key:?}: window {} has no member reading", window.limit_id);
            };
            assert_eq!(
                newest["recorded_at_ms"].as_i64(),
                Some(window.read_at_ms),
                "{key:?}: {} is read at the newest member instant",
                window.limit_id
            );
            assert_eq!(
                newest["used_fraction"].as_f64(),
                Some(window.used_fraction),
                "{key:?}: {} carries the newest reading's fraction",
                window.limit_id
            );
            assert!(
                account.members.contains(&window.member),
                "{key:?}: window read off a member"
            );
            assert_eq!(
                window.current,
                window
                    .resets_at_ms
                    .is_none_or(|resets| resets > observed_at_ms),
                "{key:?}: {} is current while its reset is ahead",
                window.limit_id
            );
            match window.remaining_fraction {
                Some(remaining) => {
                    assert!(
                        window.current,
                        "{key:?}: only a current window has a remainder"
                    );
                    assert_eq!(
                        remaining,
                        WHOLE_LIMIT - window.used_fraction,
                        "{key:?}: what is left is the whole limit less the reading"
                    );
                }
                None => assert!(!window.current, "{key:?}: a rolled window has no remainder"),
            }
        }
        let tightest = account
            .windows
            .iter()
            .filter_map(|window| window.remaining_fraction)
            .reduce(f64::min);
        assert_eq!(
            account.tightest_remaining_fraction, tightest,
            "{key:?}: the tightest window is the least left"
        );
        let served = !account.usable_members.is_empty()
            && !account
                .windows
                .iter()
                .any(|window| window.current && at_limit(window.used_fraction));
        assert_eq!(
            account.available, served,
            "{key:?}: available means a session would be served"
        );
        evidence.push(json!({
            "provider": account.provider,
            "account": account.account,
            "members": account.members,
            "available": account.available,
            "tightest_remaining_fraction": account.tightest_remaining_fraction,
            "windows": account.windows.iter().map(|window| json!({
                "limit_id": window.limit_id,
                "remaining_fraction": window.remaining_fraction,
                "resets_at_ms": window.resets_at_ms,
                "read_at_ms": window.read_at_ms,
                "member": window.member,
            })).collect::<Vec<_>>(),
        }));
    }
    run.report["account_capacity"] = json!(evidence);
    run.save();
}
