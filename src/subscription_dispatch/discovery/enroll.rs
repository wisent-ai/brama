//! Reconcile observed identities with the pool before asking for independent grants.

use super::harness::DiscoveryReport;
use crate::gateway::broker;
use crate::subscription_dispatch::usage;
use serde_json::{json, Value};

pub async fn enroll(mut report: DiscoveryReport) -> Value {
    let mut pending = match super::pending::Pending::open() {
        Ok(mut pending) => match pending.retain(&report.accounts) {
            Ok(()) => pending,
            Err(error) => {
                report.errors.push(error);
                return json!({"ok": false, "accounts": [], "observations": report.accounts, "errors": report.errors});
            }
        },
        Err(error) => {
            report.errors.push(error);
            return json!({"ok": false, "accounts": [], "observations": report.accounts, "errors": report.errors});
        }
    };
    let receipts = super::receipts::collect().await;
    if let Err(error) = pending.retain(&receipts.accounts) {
        report.accounts.extend(receipts.accounts);
        report.errors.extend(receipts.errors);
        report.errors.push(error);
        return json!({"ok": false, "accounts": [], "observations": report.accounts, "errors": report.errors});
    }
    report.accounts = pending.accounts.clone();
    report.errors.extend(receipts.errors);
    let members = match broker::list_all_subscriptions().await {
        Ok(members) => members,
        Err(error) => {
            report.errors.push(format!(
                "read subscription inventory before account enrollment: {error}"
            ));
            return json!({"ok": false, "accounts": [], "observations": report.accounts, "errors": report.errors});
        }
    };
    let mut enrolled = std::collections::BTreeMap::<String, Value>::new();
    let prior_errors = report.errors.len();
    for observation in report.accounts {
        if let Err(error) = super::provider_account(&observation.provider, &observation.account) {
            report.errors.push(error);
            continue;
        }
        let existing = members.iter().find(|member| {
            member.provider == observation.provider
                && member
                    .account
                    .as_deref()
                    .is_some_and(|account| account.eq_ignore_ascii_case(&observation.account))
                && !crate::journal::is_retired(&member.id)
        });
        let id = match existing {
            Some(member) => member.id.clone(),
            None => format!(
                "{}-{}",
                observation.provider,
                broker::slug(&observation.account)
            ),
        };
        if let Some(recorded) = usage::usage_for(&id).and_then(|entry| entry.discovery) {
            if !recorded.account.eq_ignore_ascii_case(&observation.account) {
                report.errors.push(format!(
                    "{id}: discovered account identity conflicts with its persisted address"
                ));
                continue;
            }
        }
        if crate::journal::is_retired(&id) {
            report.errors.push(format!(
                "{id}: retired by its owner; discovery cannot reinstate it"
            ));
            continue;
        }
        if let Err(error) = usage::record_discovered_account(&id, &observation, existing.is_none())
        {
            report
                .errors
                .push(format!("persist discovered account {id}: {error}"));
            continue;
        }
        if let Some(previous) = enrolled.get_mut(&id) {
            if previous["observed_at_ms"]
                .as_i64()
                .is_none_or(|at| observation.observed_at_ms >= at)
            {
                previous["source"] = json!(observation.source);
                previous["observed_at_ms"] = json!(observation.observed_at_ms);
            }
            continue;
        }
        let registration = if existing.is_none() {
            broker::register_discovered_account(
                &observation.provider,
                &id,
                &observation.account,
                &json!(observation),
            )
            .await
        } else {
            Ok(())
        };
        let error = registration.err();
        if let Some(error) = &error {
            report
                .errors
                .push(format!("register discovered account {id}: {error}"));
        }
        if let Err(error) = usage::record_registration(&id, error.clone()) {
            report
                .errors
                .push(format!("persist registration result {id}: {error}"));
        }
        enrolled.insert(
            id.clone(),
            json!({
                "id": id, "provider": observation.provider, "account": observation.account,
                "source": observation.source,
                "observed_at_ms": observation.observed_at_ms,
                "registered": error.is_none(), "error": error,
            }),
        );
    }
    for (id, row) in &mut enrolled {
        match usage::usage_for(id).and_then(|entry| entry.discovery) {
            Some(discovery) => {
                row["plan"] = json!(discovery.plan);
                row["plan_at_ms"] = json!(discovery.plan_at_ms);
                row["discovered_at_ms"] = json!(discovery.discovered_at_ms);
            }
            None => report.errors.push(format!(
                "read persisted discovery result {id}: account metadata is absent"
            )),
        }
    }
    if report.errors.len() == prior_errors {
        if let Err(error) = pending.reconciled() {
            report.errors.push(error);
        }
    }
    json!({"ok": report.errors.is_empty(), "accounts": enrolled.into_values().collect::<Vec<_>>(), "errors": report.errors})
}
