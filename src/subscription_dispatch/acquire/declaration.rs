//! The one place a subscription provider is described: `providers.json`
//! beside this file, with the operator's numbers in `numeric-provenance.json`.
//!
//! Buying, proving, handing over and leasing an account all read the
//! provider's declaration here, so adding a provider is a declaration (and,
//! in Weles, its purchase trajectory), never another branch in code. A
//! provider without a declaration, or a declaration missing a piece, is
//! refused by the missing piece.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;
use serde_json::Value;

/// How one coding-agent harness uses the provider's accounts.
#[derive(Clone, Debug, Deserialize)]
pub struct HarnessDeclaration {
    /// The harness's own name for the provider (`anthropic` in omp).
    pub provider: String,
    /// The model a session on one of the provider's subscriptions starts on,
    /// as the harness's `--model` takes it.
    pub model: Option<String>,
}

/// Everything Brama reads about one provider.
#[derive(Clone, Debug, Deserialize)]
pub struct ProviderDeclaration {
    /// The provider name Weles's purchase and authorization routes take.
    pub weles_provider: String,
    /// The name of the operator's account cap in numeric-provenance.json.
    pub accounts_max: Option<String>,
    /// The name of the operator's sessions-per-subscription limit there.
    pub sessions_per_subscription_max: String,
    /// The harnesses the provider's accounts are handed over to, by name.
    pub harnesses: BTreeMap<String, HarnessDeclaration>,
}

#[derive(Deserialize)]
struct Declarations {
    providers: BTreeMap<String, ProviderDeclaration>,
}

/// Where the declaration lives, for refusals.
pub const DECLARATION_FILE: &str = "src/subscription_dispatch/acquire/providers.json";
/// Where the operator's numbers live, for refusals.
pub const NUMBERS_FILE: &str = "src/subscription_dispatch/acquire/numeric-provenance.json";

static DECLARED: LazyLock<Declarations> = LazyLock::new(|| {
    serde_json::from_str(include_str!("providers.json"))
        .expect("providers.json beside the acquisition module is the provider declaration")
});

static STATED: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("numeric-provenance.json"))
        .expect("numeric-provenance.json beside the acquisition module is valid JSON")
});

/// Every provider with a declaration, in name order.
pub fn declared_providers() -> Vec<&'static str> {
    DECLARED.providers.keys().map(String::as_str).collect()
}

/// The declaration of `provider`, or the refusal naming what is missing.
pub fn provider(provider: &str) -> Result<&'static ProviderDeclaration, String> {
    DECLARED.providers.get(provider).ok_or_else(|| {
        format!(
            "{DECLARATION_FILE} declares no subscription provider `{provider}`; it declares {}",
            declared_providers().join(", ")
        )
    })
}

/// How `harness` uses `provider`'s accounts, or the refusal naming what is
/// missing.
pub fn harness(provider: &str, harness: &str) -> Result<&'static HarnessDeclaration, String> {
    let declared = self::provider(provider)?;
    declared.harnesses.get(harness).ok_or_else(|| {
        let named: Vec<&str> = declared.harnesses.keys().map(String::as_str).collect();
        format!(
            "{DECLARATION_FILE} declares no `{harness}` harness for `{provider}`; it declares {}",
            if named.is_empty() {
                "none".to_string()
            } else {
                named.join(", ")
            }
        )
    })
}

/// One of the operator's numbers, by the name a declaration gives it.
fn stated(name: &str) -> Result<u64, String> {
    STATED
        .get(name)
        .and_then(|entry| entry.get("value"))
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{NUMBERS_FILE} states no number `{name}`"))
}

/// The most accounts of `provider` the operator allows.
pub fn accounts_cap(provider: &str) -> Result<u64, String> {
    let declared = self::provider(provider)?;
    let name = declared.accounts_max.as_deref().ok_or_else(|| {
        format!("the operator has not authorized account purchases for {provider}")
    })?;
    stated(name).map_err(|missing| {
        format!("the operator's account cap for {provider} is not stated: {missing}")
    })
}

/// How many sessions may share one of `provider`'s subscriptions.
pub fn sessions_cap(provider: &str) -> Result<u64, String> {
    let declared = self::provider(provider)?;
    stated(&declared.sessions_per_subscription_max).map_err(|missing| {
        format!(
            "the operator's limit on sessions per {provider} subscription is not stated: {missing}"
        )
    })
}
