//! Reading the vault item that plays a role through the entitlements router:
//! one listing says which live item carries each role tag, then that item is
//! read by the id the vault gave it.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde_json::{Map, Value};

/// The v2 item schema the router answers with.
const ITEM_SCHEMA: &str = "skarbiec.item.v2";

/// The tag an item carries to play a role.
const ROLE_TAG: &str = "stado:role:";

/// One row of the router's bare `list`.
#[derive(serde::Deserialize)]
struct Listed {
    #[serde(default)]
    id: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    deleted: bool,
}

/// Every live item id carrying each role tag, from one listing. `envs` is the
/// environment the router runs with: empty inside the launcher, which already
/// carries it, and service.env's settings for a caller outside it.
pub(super) fn role_holders(
    router: &Path,
    envs: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let output = Command::new(router)
        .arg("list")
        .envs(envs)
        .output()
        .map_err(|error| {
            format!("listing the vault through the entitlements router failed: {error}")
        })?;
    if !output.status.success() {
        return Err(format!(
            "listing the vault through the entitlements router failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let rows: Vec<Listed> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("the vault listing is not a list of items: {error}"))?;
    let mut holders: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows.into_iter().filter(|row| !row.deleted) {
        for role in row.tags.iter().filter_map(|tag| tag.strip_prefix(ROLE_TAG)) {
            holders
                .entry(role.to_string())
                .or_default()
                .push(row.id.clone());
        }
    }
    Ok(holders)
}

/// The one item playing `role`. No item in the role, or several, is refused
/// with the role and the tag that selects it.
pub(super) fn item_for_role<'a>(
    holders: &'a BTreeMap<String, Vec<String>>,
    role: &str,
) -> Result<&'a str, String> {
    match holders.get(role).map(Vec::as_slice) {
        Some([one]) => Ok(one.as_str()),
        None | Some([]) => Err(format!(
            "no vault item carries {ROLE_TAG}{role}; tag the item that plays role {role} with it"
        )),
        Some(several) => Err(format!(
            "{} vault items carry {ROLE_TAG}{role}; exactly one item may play role {role}",
            several.len()
        )),
    }
}

/// The fields of one vault item, or why the router would not give them.
pub(super) fn item_fields(
    router: &Path,
    envs: &BTreeMap<String, String>,
    item: &str,
) -> Result<Map<String, Value>, String> {
    let output = Command::new(router)
        .args(["get", item])
        .envs(envs)
        .output()
        .map_err(|error| {
            format!("reading {item} through the entitlements router failed: {error}")
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(format!(
            "reading {item} through the entitlements router failed: {detail}"
        ));
    }
    let payload: Value =
        serde_json::from_slice(&output.stdout).map_err(|error| format!("{item}: {error}"))?;
    if payload.get("schema").and_then(Value::as_str) != Some(ITEM_SCHEMA) {
        return Err(format!("{item} did not return a Skarbiec v2 item"));
    }
    payload
        .get("fields")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| format!("{item} did not return a fields object"))
}

pub(super) fn field(fields: &Map<String, Value>, item: &str, name: &str) -> Result<String, String> {
    fields
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{item}/{name} is empty"))
}

/// One field of the item that plays `role`, printed bare.
pub(in crate::cli::launcher) fn role_field(
    router: &Path,
    role: &str,
    name: &str,
) -> Result<String, String> {
    role_field_in(router, &BTreeMap::new(), role, name)
}

/// One field of the item that plays `role`, read with the router running in
/// `envs` — what a command outside the launcher, such as `brama probe`, passes
/// from service.env so the router finds the vault.
pub(crate) fn role_field_in(
    router: &Path,
    envs: &BTreeMap<String, String>,
    role: &str,
    name: &str,
) -> Result<String, String> {
    let holders = role_holders(router, envs)?;
    let item = item_for_role(&holders, role)?;
    field(&item_fields(router, envs, item)?, role, name)
}
