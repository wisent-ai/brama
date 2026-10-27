//! What a refused usage reading is called, and what is written down when the
//! provider will not answer.
//!
//! Every refusal on this path has to name the same three things -- the
//! subscription, the provider, and the fact that the reading is what is now
//! missing -- so those are stated once here rather than at each of the eight
//! places that give up. The other reason this is its own file is the code a
//! refusal carries: the provider's HTTP status when it answered with one,
//! else the class the provider layer gave it, both carried as fields rather
//! than read back out of a sentence; and a refusal that was itself the ledger
//! write must not be recorded by writing to the ledger again.

use wisent_errors::{Code, Failure};

use crate::core::failure::{self, POINT_CREDENTIAL_REDEEM, POINT_PROVIDER_CALL};
use crate::subscription_dispatch::usage;
use crate::types::ProviderRefusal;

pub(super) const POINT_USAGE_LEDGER_PERSIST: &str = "brama.subscription-usage.ledger-persist";
const IMPACT_PLAN_USAGE: &str = "this subscription's current usage report";
/// What is recorded when Brama supports no free usage-report endpoint for this
/// provider credential.
///
/// This deliberately makes no claim about separately privileged organization
/// billing APIs the vendor may offer.
pub(super) const UNPUBLISHED_DETAIL: &str =
    "Brama has no supported free usage-report endpoint for this \
     provider credential; plan windows are recorded only from real traffic";

pub(super) fn scoped_failure(
    point: &str,
    code: Code,
    subscription_id: &str,
    provider: &str,
    detail: impl Into<String>,
) -> Failure {
    failure::envelope(point, code, IMPACT_PLAN_USAGE, detail)
        .with_context("subscription", subscription_id)
        .with_context("provider", provider)
}

/// A usage report the provider refused: coded by its HTTP status when a
/// refusal came back as one, else by the class the provider layer gave it.
pub(super) fn provider_failure(
    subscription_id: &str,
    provider: &str,
    class: ProviderRefusal,
    status: Option<reqwest::StatusCode>,
    detail: String,
) -> Failure {
    let code = match status.filter(|status| !status.is_success()) {
        Some(status) => Code::from_upstream_status(status.as_u16()),
        None => failure::code_for(class.contract_kind()),
    };
    scoped_failure(POINT_PROVIDER_CALL, code, subscription_id, provider, detail)
}

/// A reset offer that could not be read after a good usage report. The reset
/// reader states its refusal as one sentence (the request, the provider's
/// status and body as sent) and records no class, so the code is the fleet's
/// word for unattributed; the sentence carries what happened.
pub(super) fn reset_failure(subscription_id: &str, provider: &str, detail: String) -> Failure {
    scoped_failure(
        POINT_PROVIDER_CALL,
        Code::Unknown,
        subscription_id,
        provider,
        detail,
    )
}

pub(super) fn invalid_credential_failure(
    subscription_id: &str,
    provider: &str,
    item: &str,
    error: crate::capability::CapabilityError,
) -> Failure {
    scoped_failure(
        POINT_CREDENTIAL_REDEEM,
        Code::Config,
        subscription_id,
        provider,
        format!("read subscription credential `{item}` as UTF-8: {error}"),
    )
}

pub(super) fn record_refusal(subscription_id: &str, provider: &str, refused: Failure) -> Failure {
    match usage::record_plan_usage_failure(subscription_id, provider, &refused) {
        Ok(()) => refused,
        Err(storage) => storage,
    }
}
