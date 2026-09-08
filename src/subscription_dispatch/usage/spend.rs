//! What one provider call adds to the account that paid for it, and until when
//! a rationed account may spend nothing more.
//!
//! Both facts here are written from the same answer, which is why they are read
//! together. Brama's own measurement -- requests, failures, tokens, when the
//! account was first and last used -- is the one number this file owns outright:
//! nobody else can state it and it is always available. The block is the
//! opposite kind of statement: the provider refused this credential for quota,
//! and the refusal is turned into an instant before which nothing should try
//! again, so the dispatcher stops re-deriving that from error strings on every
//! call.
//!
//! A successful answer clears a block without waiting for its deadline. The
//! provider just served the account, which is better evidence than the estimate
//! that produced the block.

use serde::{Deserialize, Serialize};

use crate::core::failure::{self, IMPACT_CREDENTIAL_BLOCK, POINT_CREDENTIAL_BLOCK};
use crate::types::ModelResponse;

use super::{now_ms, read_ledger, with_ledger, CredentialState, UsageSource, REASON_LIMIT};

const DEFAULT_BLOCK_MS: i64 = 15 * 60 * 1_000;
const MAX_BLOCK_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Measured {
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub failures: u64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_ms: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Block {
    pub blocked_until_ms: i64,
    pub reason: String,
    pub recorded_at_ms: i64,
    /// The same refusal in the fleet's envelope, as JSON. An extra key rather
    /// than a replacement: the three above are what other tooling reads out of
    /// this file, and a reader that does not know this one ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub envelope: Option<String>,
}

/// Record one provider call against the subscription that paid for it.
pub fn record_call(subscription_id: &str, provider: &str, response: &ModelResponse) {
    record_call_from(subscription_id, provider, response, UsageSource::Traffic);
}

/// The same, saying which kind of call this was.
///
/// A caller's request and an operator's probe both measure real spend and both
/// carry the provider's plan headers, so they share every line of this. They are
/// not the same statement about the plan, though: one is a window the account
/// happened to reveal while working, the other a window somebody asked for, and
/// a row that cannot say which cannot explain why a reading exists.
pub fn record_call_from(
    subscription_id: &str,
    provider: &str,
    response: &ModelResponse,
    source: UsageSource,
) {
    let now = now_ms();
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.measured.since_ms.get_or_insert(now);
        entry.measured.last_used_ms = Some(now);
        entry.measured.requests = entry.measured.requests.saturating_add(1);
        if !response.success {
            entry.measured.failures = entry.measured.failures.saturating_add(1);
        }
        entry.measured.input_tokens = entry
            .measured
            .input_tokens
            .saturating_add(u64::from(response.input_tokens));
        entry.measured.output_tokens = entry
            .measured
            .output_tokens
            .saturating_add(u64::from(response.output_tokens));
        for reading in &response.limits {
            entry
                .limits
                .insert(reading.limit_id.clone(), reading.clone());
        }
        // An answer that carried no window says nothing about where the newest
        // one came from, so the recorded source stays as it was.
        if !response.limits.is_empty() {
            entry.usage_source = Some(source);
        }
        // A window that is no longer exhausted clears the block without waiting
        // for its deadline: the provider just answered, which is better evidence
        // than the estimate that produced the block.
        if response.success {
            entry.block = None;
            // An answer is also proof that the grant behind it is accepted, so a
            // recorded refusal is over. This is what lets a credential repaired
            // outside Brama -- the renewal loop writes the vault item directly --
            // come back on its own instead of reading as needing a sign-in
            // forever while it works. A retirement is not cleared here: one
            // stray success must not un-retire a subscription somebody retired.
            if let Some(credential) = entry.credential.as_mut() {
                if credential.state == CredentialState::NeedsReauthorization {
                    credential.state = CredentialState::Active;
                    credential.cause = None;
                    credential.recorded_at_ms = now;
                }
            }
        }
    });
}

/// Mark a subscription unusable until an instant.
///
/// The provider's own reset time is preferred over any local guess; the default
/// is used only when the answer carried no reset at all, and it is bounded so a
/// malformed header cannot retire a credential for a year.
pub fn record_block(subscription_id: &str, provider: &str, reason: &str, response: &ModelResponse) {
    let now = now_ms();
    let from_provider = response
        .limits
        .iter()
        .filter(|reading| reading.used_fraction >= 1.0 || response.limits.len() == 1)
        .filter_map(|reading| reading.resets_at_ms)
        .filter(|resets| *resets > now)
        .min();
    let until = from_provider
        .unwrap_or(now.saturating_add(DEFAULT_BLOCK_MS))
        .min(now.saturating_add(MAX_BLOCK_MS));
    // A block is a rate limit the provider stated. The sentence it stated is
    // kept in `reason` for the tooling that already reads it and in the
    // envelope for the operator who needs to know it is transient.
    let blocked = failure::envelope(
        POINT_CREDENTIAL_BLOCK,
        failure::code_for("provider_rate_limited"),
        IMPACT_CREDENTIAL_BLOCK,
        reason,
    )
    .with_context("subscription", subscription_id)
    .with_context("provider", provider);
    with_ledger(|ledger| {
        let entry = ledger
            .subscriptions
            .entry(subscription_id.to_string())
            .or_default();
        entry.provider = provider.to_string();
        entry.updated_at_ms = Some(now);
        entry.block = Some(Block {
            blocked_until_ms: until,
            reason: reason.chars().take(REASON_LIMIT).collect(),
            recorded_at_ms: now,
            envelope: Some(blocked.to_json()),
        });
    });
}

/// Whether this subscription is inside a recorded block right now.
pub fn is_blocked(subscription_id: &str) -> bool {
    let now = now_ms();
    read_ledger(|ledger| {
        ledger
            .subscriptions
            .get(subscription_id)
            .and_then(|entry| entry.block.as_ref())
            .is_some_and(|block| block.blocked_until_ms > now)
    })
}
