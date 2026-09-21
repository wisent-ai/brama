//! What the vault says about the authenticator seed on a login, asked of
//! it rather than read from it.
//!
//! Split out of `broker.rs`, which had grown past the three-hundred-line
//! limit; redemption stays there.

use super::entitlements_router_bin;

/// What Skarbiec says about the authenticator seed of every login it holds,
/// keyed by login item.
///
/// Asked of the vault, never read from it: `totp-seed-state` reports the
/// state of the field and never its value, which is exactly what a caller
/// confirming an enrolment needs. One call answers for every login, because
/// a reader that asks per member pays a full vault pass per member.
pub fn login_seed_states() -> std::collections::BTreeMap<String, String> {
    let program = entitlements_router_bin();
    let Ok(output) = std::process::Command::new(&program)
        .arg("totp-seed-state")
        .output()
    else {
        return std::collections::BTreeMap::new();
    };
    if !output.status.success() {
        return std::collections::BTreeMap::new();
    }
    let Ok(document) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        return std::collections::BTreeMap::new();
    };
    document
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let item = row.get("item").and_then(serde_json::Value::as_str)?;
                    let state = row.get("seed_state").and_then(serde_json::Value::as_str)?;
                    Some((item.to_owned(), state.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether Skarbiec holds a usable authenticator seed on one login item.
///
/// A trajectory that answers `ok` is a claim; this is the world it is
/// checked against.
pub fn login_seed_present(login_item: &str) -> bool {
    const SEED_PRESENT: &str = "present";
    login_seed_states()
        .get(login_item)
        .is_some_and(|state| state == SEED_PRESENT)
}
