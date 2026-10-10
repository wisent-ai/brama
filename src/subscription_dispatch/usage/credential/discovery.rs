//! Account discovery is durable metadata, never proof of a usable grant.

use super::super::{now_ms, write_ledger};
use super::{Credential, CredentialState};
use crate::subscription_dispatch::discovery::harness::AccountObservation;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DiscoveredAccount {
    pub account: String,
    pub plan: Option<String>,
    #[serde(default)]
    pub plan_at_ms: Option<i64>,
    pub sources: Vec<String>,
    pub discovered_at_ms: i64,
    pub observed_at_ms: i64,
    pub registration_error: Option<String>,
}

/// A failed sign-in check is not a completed browser attempt or a cooldown.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SignInCheckFailure {
    pub code: String,
    pub detail: String,
    pub at_ms: i64,
}

pub fn record_sign_in_check(
    id: &str,
    provider: &str,
    failure: Option<(&str, &str)>,
) -> Result<(), String> {
    let (_, result) = write_ledger(|ledger| {
        let entry = ledger.subscriptions.entry(id.to_owned()).or_default();
        entry.provider = provider.to_owned();
        entry.sign_in_check_failure = failure.map(|(code, detail)| SignInCheckFailure {
            code: code.to_owned(),
            detail: detail.to_owned(),
            at_ms: now_ms(),
        });
        entry.updated_at_ms = Some(now_ms());
    });
    result
}

pub fn record_discovered_account(
    id: &str,
    observation: &AccountObservation,
    needs_grant: bool,
) -> Result<(), String> {
    let (_, result) = write_ledger(|ledger| {
        let entry = ledger.subscriptions.entry(id.to_owned()).or_default();
        entry.provider = observation.provider.clone();
        let discovered = entry.discovery.get_or_insert_with(|| DiscoveredAccount {
            account: observation.account.clone(),
            plan: observation.plan.clone(),
            plan_at_ms: observation.plan.as_ref().map(|_| observation.fact_at_ms),
            sources: Vec::new(),
            discovered_at_ms: observation.observed_at_ms,
            observed_at_ms: observation.observed_at_ms,
            registration_error: None,
        });
        if !discovered.sources.contains(&observation.source) {
            discovered.sources.push(observation.source.clone());
        }
        if observation.plan.is_some()
            && discovered
                .plan_at_ms
                .is_none_or(|at| observation.fact_at_ms >= at)
        {
            discovered.plan = observation.plan.clone();
            discovered.plan_at_ms = Some(observation.fact_at_ms);
        }
        discovered.observed_at_ms = discovered.observed_at_ms.max(observation.observed_at_ms);
        entry.updated_at_ms = Some(now_ms());
        if needs_grant && entry.credential.is_none() {
            entry.credential = Some(Credential {
                state: CredentialState::NeedsReauthorization,
                cause: Some(
                    if crate::subscription_dispatch::sign_in::weles_provider(&observation.provider)
                        .is_some()
                    {
                        "account discovered; an independent Brama sign-in has not supplied a grant"
                            .into()
                    } else {
                        format!("account discovered; provider {} declares no Weles authorization capability, so no independent Brama grant was obtained", observation.provider)
                    },
                ),
                recorded_at_ms: now_ms(),
                ..Credential::default()
            });
        }
    });
    result
}

pub fn record_registration(id: &str, error: Option<String>) -> Result<(), String> {
    let (_, result) = write_ledger(|ledger| {
        if let Some(entry) = ledger.subscriptions.get_mut(id) {
            if let Some(discovery) = &mut entry.discovery {
                discovery.registration_error = error;
                entry.updated_at_ms = Some(now_ms());
            }
        }
    });
    result
}
