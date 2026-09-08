//! What a refused usage reading is called, and what is written down when the
//! provider will not answer.
//!
//! Every refusal on this path has to name the same three things -- the
//! subscription, the provider, and the fact that the reading is what is now
//! missing -- so those are stated once here rather than at each of the eight
//! places that give up. The other reason this is its own file is the reading
//! of the provider's own words: an HTTP status recovered from a detail string
//! decides the code, an authentication refusal is the one refusal worth
//! retrying with a fresh token, and a refusal that was itself the ledger write
//! must not be recorded by writing to the ledger again.

use wisent_errors::{Code, Failure};

use crate::core::failure::{self, POINT_CREDENTIAL_REDEEM, POINT_PROVIDER_CALL};
use crate::subscription_dispatch::usage;

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

fn provider_status(detail: &str) -> Option<u16> {
    let (_, status) = detail.split_once(" returned HTTP ")?;
    let digits = status
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    digits.parse().ok()
}

pub(super) fn provider_failure(subscription_id: &str, provider: &str, detail: String) -> Failure {
    let code = provider_status(&detail)
        .filter(|status| !(200..300).contains(status))
        .map(Code::from_upstream_status)
        .unwrap_or_else(|| failure::code_for_message(&detail, "provider_failure"));
    scoped_failure(POINT_PROVIDER_CALL, code, subscription_id, provider, detail)
}

pub(super) fn invalid_credential_failure(
    subscription_id: &str,
    provider: &str,
    item: &str,
) -> Failure {
    scoped_failure(
        POINT_CREDENTIAL_REDEEM,
        Code::Config,
        subscription_id,
        provider,
        format!("subscription credential `{item}` is not valid UTF-8"),
    )
}

pub(super) fn record_refusal(subscription_id: &str, provider: &str, refused: Failure) -> Failure {
    match usage::record_plan_usage_failure(subscription_id, provider, &refused) {
        Ok(()) => refused,
        Err(storage) => storage,
    }
}

pub(super) fn is_provider_authentication(detail: &str) -> bool {
    detail
        .split_once(':')
        .is_some_and(|(kind, _)| kind.trim() == "provider_authentication")
}
