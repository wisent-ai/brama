//! Which providers this host may authenticate as, and which aliases can use
//! them. Three files have to agree before one alias answers: policy.json
//! grants a provider resource, capability-routes.json says where that
//! resource lives in the vault, and inference-routes.json points an alias at
//! that provider. Any one alone looks correct, so each is printed against the
//! other two.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{moment, read_json, Layout};

const PROVIDER_PURPOSE: &str = "brama.provider.authenticate";
const BEST_ALIAS: &str = "best";

fn normalize(provider: &str) -> String {
    provider.trim().to_lowercase().replace('_', "-")
}

/// The providers policy.json and the routes table agree on, in the spelling
/// a route uses. A capability is issued at final use, so the durable
/// agreement is the exact resource both files share.
pub(super) fn print_policy_grants(layout: &Layout, config_dir: &Path) -> BTreeSet<String> {
    println!("\n=== policy grants against authority routes");
    let policy_path = config_dir.join("policy.json");
    let mut granted = BTreeSet::new();
    match read_json(&policy_path) {
        Some(policy) => {
            for rule in policy.pointer("/roles/brama-runtime").and_then(Value::as_array).into_iter().flatten() {
                if rule.get("purpose").and_then(Value::as_str) == Some(PROVIDER_PURPOSE) {
                    if let Some(resource) = rule.get("resource").and_then(Value::as_str) {
                        granted.insert(resource.to_string());
                    }
                }
            }
            println!("  policy.json written {}", moment(&policy_path));
            let listed: Vec<&str> = granted.iter().map(String::as_str).collect();
            println!("  granted provider resources ({}): {}", granted.len(), if listed.is_empty() { "none".into() } else { listed.join(", ") });
        }
        None => println!("  {}: absent", policy_path.display()),
    }
    let routes_path = match (layout.settings.get("SKARBIEC_CAPABILITY_ROUTES_FILE"), layout.settings.get("SKARBIEC_VAULT_FILE")) {
        (Some(routes), _) => PathBuf::from(routes),
        (None, Some(vault)) => Path::new(vault).parent().unwrap_or(Path::new("/")).join("capability-routes.json"),
        (None, None) => layout.home.join(".config/skarbiec/capability-routes.json"),
    };
    let mut routed = BTreeSet::new();
    match read_json(&routes_path) {
        Some(document) => {
            let table = document.get("routes").cloned().unwrap_or(document);
            for (resource, coordinate) in table.as_object().into_iter().flatten() {
                if coordinate.get("item").is_some_and(Value::is_string) && coordinate.get("field").is_some_and(Value::is_string) {
                    routed.insert(resource.clone());
                }
            }
            println!("  {}", routes_path.display());
            let unrouted: Vec<&str> = granted.difference(&routed).map(String::as_str).collect();
            if !unrouted.is_empty() {
                println!("  granted but unrouted: {}", unrouted.join(", "));
            }
            let ungranted: Vec<&str> = routed.difference(&granted).map(String::as_str).filter(|resource| resource.starts_with("provider:")).collect();
            if !ungranted.is_empty() {
                println!("  routed without a provider grant: {}", ungranted.join(", "));
            }
        }
        None => println!("  {}: absent", routes_path.display()),
    }
    granted
        .intersection(&routed)
        .filter_map(|resource| match resource.split(':').collect::<Vec<_>>().as_slice() {
            ["provider", provider] => Some(normalize(provider)),
            _ => None,
        })
        .collect()
}

/// Every alias route against the providers policy and routes agree on.
pub(super) fn print_alias_routes(layout: &Layout, providers: &BTreeSet<String>) {
    println!("\n=== alias routes against routed provider grants");
    let routes_path = layout
        .settings
        .get("BRAMA_INFERENCE_ROUTES_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| layout.home.join(".config/brama/inference-routes.json"));
    let Some(document) = read_json(&routes_path) else {
        println!("  {}: absent", routes_path.display());
        return;
    };
    println!("  {}", routes_path.display());
    let deployments: Vec<&str> = document
        .get("deployments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .collect();
    for (alias, route) in document.get("routes").and_then(Value::as_object).into_iter().flatten() {
        let route = route.as_str().unwrap_or("");
        let provider = route.split('/').next().unwrap_or("");
        // A bare destination naming a declared deployment is served by the
        // gateway as `local-openai/<deployment>`.
        let verdict = if route == BEST_ALIAS {
            "exempt: a subscription pays for best"
        } else if !route.contains('/') && deployments.contains(&route) {
            "local deployment"
        } else if !route.contains('/') {
            "REFUSED: names no provider and no declared deployment"
        } else if providers.contains(provider) {
            "ok"
        } else {
            "REFUSED: no routed provider grant"
        };
        println!("    {alias} -> {route} [{verdict}]");
    }
    println!("  deployments: {}", if deployments.is_empty() { "none".into() } else { deployments.join(", ") });
}
