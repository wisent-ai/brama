//! The gateway's nonsecret ingress and provider policy, read from the
//! product-owned control document (`services.brama`) and refused unless it is
//! complete: `allowed_models` names exactly the aliases `model_aliases`
//! routes, every alias Brama itself answers to is among them, every route is a
//! `provider/model` (or `best`), and `required_provider_capabilities` is a
//! non-empty list of distinct providers.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::num::NonZeroU64;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde_json::{Map, Value};

/// The aliases every Brama gateway answers to; a control document that does
/// not route each of them is incomplete.
const REQUIRED_ALIASES: &[&str] = &[
    "best",
    "wisent-backend",
    "wisent-backend/evaluation",
    "wisent-backend/embeddings",
    "wisent-backend/moderation",
    "weles",
];
/// The alias family served by the Wisent backend.
const BACKEND_ALIAS: &str = "wisent-backend";

fn write_new(path: &Path, text: &str) -> Result<(), String> {
    // Owner read and write only, as the platform spells it.
    let owner_only = u32::from(libc::S_IRUSR | libc::S_IWUSR);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(owner_only)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn clean(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|text| !text.is_empty() && text.trim() == *text)
}

pub(super) struct Outputs<'a> {
    pub(super) allowed: &'a Path,
    pub(super) aliases: &'a Path,
    pub(super) backend: &'a Path,
    pub(super) lifetime: &'a Path,
}

pub(super) fn check(config: &Path, out: &Outputs) -> Result<(), String> {
    let document: Value = std::fs::read(config)
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        .map_err(|error| format!("{}: {error}", config.display()))?;
    let file = config.display();
    let Some(policy) = document.pointer("/services/brama") else {
        return Err(format!("{file} declares no services.brama policy"));
    };
    let field = |name: &str| {
        policy
            .get(name)
            .cloned()
            .ok_or_else(|| format!("services.brama policy is incomplete: {name}"))
    };
    let allowed_models = field("allowed_models")?;
    let aliases = field("model_aliases")?;
    let providers = field("required_provider_capabilities")?;
    let lifetime = field("capability_lifetime_seconds")?;
    let lifetime = lifetime
        .as_u64()
        .and_then(NonZeroU64::new)
        .ok_or_else(|| {
            format!(
                "services.brama.capability_lifetime_seconds must be a positive whole number of seconds in {file}"
            )
        })?;
    let Some(allowed_models) = allowed_models.as_array() else {
        return Err(format!(
            "services.brama.allowed_models must be a list in {file}"
        ));
    };
    let required: BTreeSet<String> = REQUIRED_ALIASES
        .iter()
        .map(|alias| alias.to_string())
        .collect();
    let malformed: Vec<&Value> = allowed_models
        .iter()
        .filter(|value| clean(value).is_none())
        .collect();
    let names: Vec<&str> = allowed_models.iter().filter_map(Value::as_str).collect();
    let allowed: BTreeSet<String> = names.iter().map(|name| name.to_string()).collect();
    // A name seen again is a duplicate.
    let mut seen = BTreeSet::new();
    let duplicated: BTreeSet<&str> = names
        .iter()
        .copied()
        .filter(|name| !seen.insert(*name))
        .collect();
    let Some(alias_map) = aliases.as_object() else {
        return Err(format!(
            "services.brama.model_aliases must be an object of alias to route in {file}"
        ));
    };
    let alias_names: BTreeSet<String> = alias_map.keys().cloned().collect();
    if !malformed.is_empty()
        || !duplicated.is_empty()
        || allowed != alias_names
        || !required.is_subset(&allowed)
    {
        return Err(format!(
            "services.brama.allowed_models must match model_aliases and include the required aliases; file={file}; missing_required={:?}; missing_from_allowed={:?}; missing_from_aliases={:?}; malformed={malformed:?}; duplicated={duplicated:?}",
            required.difference(&allowed).collect::<Vec<_>>(),
            alias_names.difference(&allowed).collect::<Vec<_>>(),
            allowed.difference(&alias_names).collect::<Vec<_>>(),
        ));
    }
    let malformed_routes: Map<String, Value> = alias_map
        .iter()
        .filter(|(_, route)| {
            clean(route).is_none_or(|route| !route.contains('/') && route != "best")
        })
        .map(|(alias, route)| (alias.clone(), route.clone()))
        .collect();
    if !malformed_routes.is_empty() {
        return Err(format!(
            "services.brama.model_aliases contains malformed provider/model routes; file={file}; malformed={}",
            Value::Object(malformed_routes)
        ));
    }
    let Some(provider_list) = providers.as_array() else {
        return Err(format!(
            "services.brama.required_provider_capabilities must be a list in {file}"
        ));
    };
    let distinct: BTreeSet<&str> = provider_list.iter().filter_map(clean).collect();
    if provider_list.is_empty() || distinct.len() != provider_list.len() {
        return Err("services.brama.required_provider_capabilities must be a non-empty unique provider list".into());
    }
    write_new(out.allowed, &names.join(","))?;
    let sorted: BTreeMap<&String, &Value> = alias_map.iter().collect();
    write_new(
        out.aliases,
        &serde_json::to_string(&sorted).expect("aliases serialise"),
    )?;
    let backend: BTreeSet<&str> = REQUIRED_ALIASES
        .iter()
        .copied()
        .filter(|alias| *alias == BACKEND_ALIAS || alias.starts_with(&format!("{BACKEND_ALIAS}/")))
        .collect();
    write_new(
        out.backend,
        &serde_json::to_string(&backend).expect("list serialises"),
    )?;
    write_new(out.lifetime, &lifetime.to_string())
}

/// The first route registry: exactly the validated launch aliases, in the
/// three fields the registry reader accepts.
pub(super) fn seed_routes(path: &Path, aliases: &str) -> Result<(), String> {
    let routes: Value =
        serde_json::from_str(aliases).map_err(|error| format!("BRAMA_MODEL_ALIASES: {error}"))?;
    let document = brama::core::inference_routes::fresh_registry(routes);
    let text = serde_json::to_string_pretty(&document).expect("routes serialise") + "\n";
    write_new(path, &text)
}
