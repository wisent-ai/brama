//! Discover operator account identities without importing a harness's credentials.

mod enroll;
pub mod harness;
mod pending;
mod receipts;
pub use enroll::enroll;

use crate::subscription_dispatch::acquire::declaration;

/// Provider names belong to the declared harness interface, not member ranks.
pub fn provider_for_harness(harness: &str, name: &str) -> Result<&'static str, String> {
    let mut matching = declaration::declared_providers()
        .into_iter()
        .filter(|provider| {
            declaration::provider(provider).is_ok_and(|declared| {
                declared
                    .harnesses
                    .get(harness)
                    .is_some_and(|entry| entry.provider == name)
            })
        });
    let provider = matching.next().ok_or_else(|| {
        format!("no subscription provider declares {harness} account provider {name}")
    })?;
    if matching.next().is_some() {
        return Err(format!(
            "multiple subscription providers declare {harness} account provider {name}"
        ));
    }
    Ok(provider)
}

pub fn provider_account(provider: &str, account: &str) -> Result<(), String> {
    declaration::provider(provider)?;
    let valid = account.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty() && !domain.is_empty() && !domain.contains('@')
    }) && !account.chars().any(char::is_whitespace);
    if !valid {
        return Err(format!(
            "provider {provider}: discovery requires an account email address"
        ));
    }
    Ok(())
}

/// Enrollment and independent-grant renewal share the same local and remote path.
pub async fn discover(report: harness::DiscoveryReport) -> serde_json::Value {
    let mut answer = enroll(report).await;
    answer["credentials"] = match super::refresh_sweep::sweep().await {
        Ok(report) => report,
        Err(error) => {
            answer["ok"] = serde_json::json!(false);
            serde_json::json!({"error": error})
        }
    };
    answer
}
