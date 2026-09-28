//! The runtime subscription catalog the gateway starts with.
//!
//! Which subscriptions exist is read off the vault items, not out of a
//! manifest: `brama:subscription` marks one, `brama:provider:` and
//! `brama:id:` name it, `brama:login:` names the Weles account that can renew
//! it. A subscription in the vault is in the rotation for every caller, so a
//! row names no agent. The runtime policy still has to allow the exact
//! provider resource; no capability is issued here, because every credential
//! use asks the authority for a fresh one.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{json, Value};

const SUBSCRIPTION_TAG: &str = "brama:subscription";
const PROVIDER_TAG: &str = "brama:provider:";
const SUBSCRIPTION_ID_TAG: &str = "brama:id:";
const LOGIN_TAG: &str = "brama:login:";
const AUTHENTICATE: &str = "brama.provider.authenticate";

fn read(path: &Path) -> Result<Value, String> {
    std::fs::read(path)
        .map_err(|error| format!("{}: {error}", path.display()))
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display())))
}

fn tag_value<'a>(tags: &'a [&'a str], prefix: &str) -> Option<&'a str> {
    tags.iter().find_map(|tag| tag.strip_prefix(prefix).filter(|rest| !rest.is_empty()))
}

pub(super) fn build(available: &Path, policy: &Path, output: &Path) -> Result<(), String> {
    let items = read(available)?;
    let policy = read(policy)?;
    let allowed: BTreeSet<(String, String)> = policy
        .pointer("/roles/brama-runtime")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|rule| rule.is_object())
        .map(|rule| {
            let text = |key: &str| rule.get(key).and_then(Value::as_str).unwrap_or("").to_string();
            (text("purpose"), text("resource"))
        })
        .collect();
    let mut catalog = Vec::new();
    let mut unnamed = Vec::new();
    for item in items.as_array().into_iter().flatten() {
        if !item.is_object() || item.get("deleted").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        let Some(name) = item.get("id").and_then(Value::as_str) else { continue };
        let tags: Vec<&str> = item.get("tags").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        if !tags.contains(&SUBSCRIPTION_TAG) {
            continue;
        }
        let provider = tag_value(&tags, PROVIDER_TAG);
        let subscription = tag_value(&tags, SUBSCRIPTION_ID_TAG);
        let login = tag_value(&tags, LOGIN_TAG);
        let (Some(provider), Some(subscription)) = (provider, subscription) else {
            let missing: Vec<String> = [(PROVIDER_TAG, provider), (SUBSCRIPTION_ID_TAG, subscription)]
                .iter()
                .filter(|(_, value)| value.is_none())
                .map(|(prefix, _)| format!("{prefix}<value>"))
                .collect();
            eprintln!(
                "skipping {name}: carries {SUBSCRIPTION_TAG} but no {} tag, so nothing declares what it serves; tag it and it is served again",
                missing.join(" and no ")
            );
            continue;
        };
        let provider = provider.trim().to_lowercase().replace('_', "-");
        let resource = format!("provider:{provider}:{subscription}");
        if !allowed.contains(&(AUTHENTICATE.to_string(), resource.clone())) {
            eprintln!(
                "skipping {name}: the runtime policy has no {AUTHENTICATE} rule for {resource}, so no capability route maps this resource to a vault item and field; the policy is generated from the vault's tags when the release is installed, so an item added since then is served again after the next install of this release on this host"
            );
            unnamed.push(resource);
            continue;
        }
        catalog.push(json!({ "id": subscription, "provider": provider, "status": "active", "login_item": login }));
    }
    let document = serde_json::to_string(&json!({ "items": catalog })).expect("catalog serialises");
    std::fs::write(output, document).map_err(|error| format!("{}: {error}", output.display()))?;
    if !unnamed.is_empty() {
        unnamed.sort();
        eprintln!(
            "{} subscription(s) the runtime policy does not name are not served: {}; install this release again on this host to regenerate the policy from the vault's current tags",
            unnamed.len(),
            unnamed.join(", ")
        );
    }
    eprintln!("{} subscription(s) served by this gateway", catalog.len());
    Ok(())
}
