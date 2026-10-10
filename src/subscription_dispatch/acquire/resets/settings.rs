//! The operator's saved-reset policy per provider, as he states it.
//!
//! When Brama may spend a saved reset by itself is the operator's decision:
//! how many credits to keep, how long a window must still be blocked before a
//! credit is worth spending on it, and how close to its expiry a credit is
//! spent rather than lost. No such number is Brama's. The operator records his
//! settings with `brama subscription reset-policy set` (or Brama Desktop) on
//! the gateway that holds the pool, and they are kept in
//! `$BRAMA_STATE_DIR/reset-policy.json`, replaced whole in one rename. The
//! automatic redemption of a provider whose policy was never stated is
//! refused by name with the command that states it; a reset the operator asks
//! for by hand needs none.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One provider's policy, with who stated it, why and when.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetPolicy {
    /// Credits that are never spent automatically.
    pub keep_credits: u64,
    /// A window must stay blocked longer than this before a credit is spent on it.
    pub min_blocked_minutes: u64,
    /// A credit expiring within this many hours is spent rather than lost.
    pub salvage_horizon_hours: u64,
    pub reason: String,
    pub stated_at: String,
}

fn path() -> PathBuf {
    crate::journal::state_dir().join("reset-policy.json")
}

fn lock(provider: &str) -> Result<std::fs::File, String> {
    let file = path().with_extension("lock");
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "create saved-reset policy directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(&file)
        .map_err(|error| {
            format!(
                "open saved-reset policy lock {} for {provider}: {error}",
                file.display()
            )
        })?;
    lock.try_lock().map_err(|error| {
        format!(
            "saved-reset policy mutation for {provider} cannot acquire exclusive lock {}: {error}",
            file.display()
        )
    })?;
    Ok(lock)
}

/// Every provider's stated policy; none when nothing was ever stated.
pub fn all() -> Result<BTreeMap<String, ResetPolicy>, String> {
    let file = path();
    match std::fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "decode saved-reset policy {}: {error}; the existing policy was not replaced",
                file.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(format!(
            "the saved-reset policy in {} cannot be read: {error}",
            file.display()
        )),
    }
}

/// The policy the operator stated for `provider`, or why there is none.
pub fn for_provider(provider: &str) -> Result<ResetPolicy, String> {
    all()?.remove(provider).ok_or_else(|| {
        format!(
            "the operator has stated no saved-reset policy for {provider}, so no reset is redeemed automatically; `brama subscription reset-policy set {provider} --keep-credits N --min-blocked-minutes N --salvage-horizon-hours N --reason TEXT` records his settings on the gateway that holds the pool (Brama Desktop: Saved resets)"
        )
    })
}

fn write(policies: &BTreeMap<String, ResetPolicy>) -> Result<(), String> {
    let file = path();
    let staged = file.with_extension("json.next");
    let bytes = serde_json::to_vec_pretty(policies)
        .map_err(|error| format!("the saved-reset policy cannot be encoded: {error}"))?;
    std::fs::write(&staged, bytes).map_err(|error| {
        format!(
            "the saved-reset policy cannot be written: {}: {error}",
            staged.display()
        )
    })?;
    std::fs::rename(&staged, &file).map_err(|error| {
        format!(
            "the saved-reset policy cannot be written: rename {} to {}: {error}",
            staged.display(),
            file.display()
        )
    })
}

/// Record `policy` as `provider`'s, replacing the one before; answers it.
pub fn set(provider: &str, policy: ResetPolicy) -> Result<ResetPolicy, String> {
    super::super::declaration::provider(provider)?
        .resets
        .as_ref()
        .ok_or_else(|| format!("provider {provider} declares no saved-reset report, so it has no reset policy to state"))?;
    if policy.reason.trim().is_empty() {
        return Err("--reason must say why this saved-reset policy is stated".into());
    }
    let _lock = lock(provider)?;
    let mut policies = all()?;
    policies.insert(provider.to_string(), policy.clone());
    write(&policies)?;
    Ok(policy)
}

/// Remove `provider`'s policy, so it is no longer redeemed automatically;
/// answers the policy removed.
pub fn clear(provider: &str) -> Result<ResetPolicy, String> {
    let _lock = lock(provider)?;
    let mut policies = all()?;
    let removed = policies.remove(provider).ok_or_else(|| {
        format!("no saved-reset policy is stated for {provider}; `brama subscription reset-policy show` lists every stated one")
    })?;
    write(&policies)?;
    Ok(removed)
}
