//! What the vault says about the authenticator seed on a login, asked of
//! it rather than read from it.
//!
//! Split out of `broker.rs`, which had grown past the three-hundred-line
//! limit; redemption stays there.

use super::entitlements_router_bin;

const ERROR_PREVIEW_CHARS: usize = 512;

/// What Skarbiec says about the authenticator seed of every login it holds,
/// keyed by login item.
///
/// Asked of the vault, never read from it: `totp-seed-state` reports the
/// state of the field and never its value, which is exactly what a caller
/// confirming an enrolment needs. One call answers for every login, because
/// a reader that asks per member pays a full vault pass per member.
pub fn login_seed_states() -> Result<std::collections::BTreeMap<String, String>, String> {
    let program = entitlements_router_bin();
    let output = std::process::Command::new(&program)
        .arg("totp-seed-state")
        .output()
        .map_err(|error| format!("{program} totp-seed-state could not start: {error}"))?;
    if !output.status.success() {
        let detail: String = String::from_utf8_lossy(&output.stderr).chars().take(ERROR_PREVIEW_CHARS).collect();
        return Err(format!("{program} totp-seed-state exited {}: {detail}", output.status));
    }
    let document: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("{program} totp-seed-state returned invalid JSON: {error}"))?;
    let rows = document.get("rows").and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("{program} totp-seed-state returned no rows array"))?;
    rows.iter().enumerate().map(|(index, row)| {
        let item = row.get("item").and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{program} totp-seed-state row {index} has no item"))?;
        let state = row.get("seed_state").and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{program} totp-seed-state returned no seed_state for {item}"))?;
        Ok((item.to_owned(), state.to_owned()))
    }).collect()
}

/// Whether Skarbiec holds a usable authenticator seed on one login item.
///
/// A trajectory that answers `ok` is a claim; this is the world it is
/// checked against.
pub fn login_seed_present(login_item: &str) -> Result<bool, String> {
    let states = login_seed_states()?;
    let state = states.get(login_item)
        .ok_or_else(|| format!("totp-seed-state returned no row for resolved login {login_item}"))?;
    Ok(state == "present")
}
