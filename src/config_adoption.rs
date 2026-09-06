use std::collections::{BTreeMap, HashMap, HashSet};
use std::marker::PhantomData;
use std::path::Path;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::core::inference_routes::{self, RouteImport, RouteImportDisposition, RouteImportResult};
use crate::core::server::{
    alias_requires_direct_capability, alias_route_shape_supported, valid_alias, BEST_ALIAS,
};
use crate::gateway::broker;
use crate::providers::adapter;

const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;
const MAX_SOURCE_NAME_CHARACTERS: usize = 512;
const MAX_AGENT_ID_CHARACTERS: usize = 128;
const MAX_ALIASES: usize = 1024;
const MAX_DEPLOYMENTS: usize = 128;
const MAX_DESTINATION_CHARACTERS: usize = 512;

fn schema_version() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SourceEndpoint {
    host: String,
    port: u16,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SourceAdapter {
    name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SourceDeployment {
    name: String,
    #[serde(default)]
    adapters: Vec<SourceAdapter>,
    endpoint: SourceEndpoint,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SourceRegistry {
    #[serde(default = "schema_version")]
    schema_version: u32,
    #[serde(default)]
    deployments: Vec<SourceDeployment>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    routes: BTreeMap<String, String>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    fallbacks: BTreeMap<String, Vec<String>>,
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self {
            schema_version: schema_version(),
            deployments: Vec::new(),
            routes: BTreeMap::new(),
            fallbacks: BTreeMap::new(),
        }
    }
}

fn deserialize_unique_map<'de, D, T>(deserializer: D) -> Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct UniqueMapVisitor<T>(PhantomData<T>);

    impl<'de, T> Visitor<'de> for UniqueMapVisitor<T>
    where
        T: Deserialize<'de>,
    {
        type Value = BTreeMap<String, T>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("an object with unique keys")
        }

        fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut values = BTreeMap::new();
            while let Some((key, value)) = access.next_entry::<String, T>()? {
                if values.insert(key.clone(), value).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate configuration key '{key}'"
                    )));
                }
            }
            Ok(values)
        }
    }

    deserializer.deserialize_map(UniqueMapVisitor(PhantomData))
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdoptionDisposition {
    Importable,
    Imported,
    Unchanged,
    Conflicting,
    Rejected,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdoptionProviderIdentity {
    pub provider: String,
    pub acquisition: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdoptionSubscriptionIdentity {
    pub provider: String,
    pub subscription_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdoptionCandidate {
    pub alias: String,
    pub primary: String,
    pub fallbacks: Vec<String>,
    pub deployments: Vec<String>,
    pub existing_primary: Option<String>,
    pub existing_fallbacks: Vec<String>,
    pub disposition: AdoptionDisposition,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdoptionPreview {
    pub schema_version: u32,
    pub source: String,
    pub destination: String,
    pub agent_id: String,
    pub providers: Vec<AdoptionProviderIdentity>,
    pub subscriptions: Vec<AdoptionSubscriptionIdentity>,
    pub subscription_discovery: String,
    pub candidates: Vec<AdoptionCandidate>,
    pub unreferenced_deployments: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdoptionItemResult {
    pub alias: String,
    pub disposition: AdoptionDisposition,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdoptionResult {
    pub schema_version: u32,
    pub source: String,
    pub destination: String,
    pub items: Vec<AdoptionItemResult>,
    pub imported: usize,
    pub unchanged: usize,
    pub conflicting: usize,
    pub rejected: usize,
    pub routes: Value,
}

pub fn default_destination() -> Result<std::path::PathBuf, String> {
    if let Some(path) = inference_routes::configured_path() {
        return Ok(path);
    }
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "HOME is required when BRAMA_INFERENCE_ROUTES_FILE is not set".to_string()
        })?;
    Ok(std::path::PathBuf::from(home)
        .join(".config")
        .join("brama")
        .join("inference-routes.json"))
}

pub async fn preview_document(
    encoded: &str,
    source_name: &str,
    destination: &Path,
    agent_id: &str,
) -> Result<AdoptionPreview, String> {
    validate_request_identity(source_name, agent_id)?;
    let source = parse_source(encoded)?;
    validate_source(&source)?;
    let source_value = serde_json::to_value(&source)
        .map_err(|error| format!("cannot encode imported configuration: {error}"))?;
    inference_routes::validate_document(&source_value)?;

    let destination_value = if destination_exists(destination)? {
        inference_routes::snapshot(destination)?
    } else {
        serde_json::json!({
            "schema_version": 1,
            "deployments": [],
            "routes": {},
            "fallbacks": {},
        })
    };
    inference_routes::validate_document(&destination_value)?;

    let configured_providers = broker::configured_provider_capabilities();
    let acquisition = if broker::local_provider_credentials_enabled() {
        "standalone_runtime"
    } else {
        "skarbiec_acquisition"
    };
    let mut providers = configured_providers
        .iter()
        .cloned()
        .map(|provider| AdoptionProviderIdentity {
            provider,
            acquisition: acquisition.to_string(),
        })
        .collect::<Vec<_>>();
    providers.sort_by(|left, right| left.provider.cmp(&right.provider));

    let discovered_subscriptions = broker::discover_subscriptions(agent_id).await;
    let (mut subscriptions, subscription_discovery) = match &discovered_subscriptions {
        Ok(entries) => (
            entries
                .iter()
                .map(|entry| AdoptionSubscriptionIdentity {
                    provider: entry.provider.clone(),
                    subscription_id: entry.id.clone(),
                    status: entry.status.clone(),
                })
                .collect::<Vec<_>>(),
            "available".to_string(),
        ),
        Err(error) => (Vec::new(), error.clone()),
    };
    subscriptions.sort_by(|left, right| {
        (&left.provider, &left.subscription_id).cmp(&(&right.provider, &right.subscription_id))
    });
    subscriptions.dedup_by(|left, right| {
        left.provider == right.provider && left.subscription_id == right.subscription_id
    });

    let destination_routes = destination_value
        .get("routes")
        .and_then(Value::as_object)
        .ok_or_else(|| "inference routes.routes must be an object".to_string())?;
    let destination_fallbacks = destination_value
        .get("fallbacks")
        .and_then(Value::as_object)
        .ok_or_else(|| "inference routes.fallbacks must be an object".to_string())?;
    let destination_deployments = deployments_by_name(&destination_value)?;
    let source_deployments = source_deployments_by_name(&source)?;

    let mut referenced = HashSet::new();
    let mut candidates = Vec::with_capacity(source.routes.len());
    for (alias, primary) in &source.routes {
        let fallbacks = source.fallbacks.get(alias).cloned().unwrap_or_default();
        let existing_primary = destination_routes
            .get(alias)
            .and_then(Value::as_str)
            .map(str::to_string);
        let existing_fallbacks = destination_fallbacks
            .get(alias)
            .map(|value| {
                value
                    .as_array()
                    .ok_or_else(|| format!("fallback route '{alias}' must be an array"))?
                    .iter()
                    .map(|destination| {
                        destination
                            .as_str()
                            .map(str::to_string)
                            .ok_or_else(|| format!("fallback route '{alias}' must contain strings"))
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        let mut deployment_names = Vec::new();
        for destination_name in std::iter::once(primary).chain(fallbacks.iter()) {
            if destination_name != BEST_ALIAS && !destination_name.contains('/') {
                referenced.insert(destination_name.clone());
                deployment_names.push(destination_name.clone());
            }
        }
        deployment_names.sort();
        deployment_names.dedup();

        let mut disposition = AdoptionDisposition::Importable;
        let mut detail = "ready to persist through Brama's route registry".to_string();
        for destination_name in std::iter::once(primary).chain(fallbacks.iter()) {
            let resolved = if destination_name == BEST_ALIAS || destination_name.contains('/') {
                destination_name.clone()
            } else {
                format!("local-openai/{destination_name}")
            };
            if !alias_route_shape_supported(alias, &resolved) {
                disposition = AdoptionDisposition::Rejected;
                detail = format!("route '{resolved}' is not supported for alias '{alias}'");
                break;
            }
            if resolved == BEST_ALIAS {
                match &discovered_subscriptions {
                    Ok(entries) if entries.is_empty() => {
                        disposition = AdoptionDisposition::Rejected;
                        detail =
                            format!("agent '{agent_id}' has no discoverable Skarbiec subscription");
                        break;
                    }
                    Err(error) => {
                        disposition = AdoptionDisposition::Rejected;
                        detail = error.clone();
                        break;
                    }
                    Ok(_) => {}
                }
            } else if alias_requires_direct_capability(alias, &resolved) {
                let provider = adapter::provider_id_from_route(&resolved)
                    .ok_or_else(|| format!("route '{resolved}' has no supported provider"))?;
                if adapter::provider_requires_credential(provider)
                    && !configured_providers.contains(provider)
                {
                    disposition = AdoptionDisposition::Rejected;
                    detail =
                        format!("provider '{provider}' has no configured Brama acquisition route");
                    break;
                }
            }
        }

        if disposition != AdoptionDisposition::Rejected {
            if let Some(name) = deployment_names.iter().find(|name| {
                source_deployments
                    .get(name.as_str())
                    .is_some_and(|source_deployment| {
                        destination_deployments
                            .get(name.as_str())
                            .is_some_and(|existing| existing != source_deployment)
                    })
            }) {
                disposition = AdoptionDisposition::Conflicting;
                detail = format!(
                    "deployment '{name}' already exists with a different endpoint or adapter"
                );
            } else if existing_primary.is_some() {
                if existing_primary.as_deref() == Some(primary.as_str())
                    && existing_fallbacks == fallbacks
                {
                    disposition = AdoptionDisposition::Unchanged;
                    detail = "the destination already has this route chain".to_string();
                } else {
                    disposition = AdoptionDisposition::Conflicting;
                    detail = "the destination already has a different route chain".to_string();
                }
            }
        }

        candidates.push(AdoptionCandidate {
            alias: alias.clone(),
            primary: primary.clone(),
            fallbacks,
            deployments: deployment_names,
            disposition,
            existing_primary,
            existing_fallbacks,
            detail,
        });
    }

    let mut unreferenced_deployments = source_deployments
        .keys()
        .filter(|name| !referenced.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    unreferenced_deployments.sort();

    Ok(AdoptionPreview {
        schema_version: 1,
        source: source_name.to_string(),
        destination: destination.display().to_string(),
        agent_id: agent_id.to_string(),
        providers,
        subscriptions,
        subscription_discovery,
        candidates,
        unreferenced_deployments,
    })
}

pub async fn apply_document(
    encoded: &str,
    source_name: &str,
    destination: &Path,
    agent_id: &str,
    selected_aliases: &[String],
    replace_alias_conflicts: bool,
) -> Result<AdoptionResult, String> {
    let preview = preview_document(encoded, source_name, destination, agent_id).await?;
    if selected_aliases.len() > MAX_ALIASES {
        return Err(format!("at most {MAX_ALIASES} aliases may be selected"));
    }
    let selected = selected_aliases
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if selected.len() != selected_aliases.len() {
        return Err("selected aliases must be unique".to_string());
    }
    let known = preview
        .candidates
        .iter()
        .map(|candidate| candidate.alias.as_str())
        .collect::<HashSet<_>>();
    if let Some(alias) = selected.iter().find(|alias| !known.contains(*alias)) {
        return Err(format!(
            "selected alias '{}' is not present in the source",
            alias
        ));
    }

    let source = parse_source(encoded)?;
    let source_deployments = source_deployments_by_name(&source)?;
    let mut imports = Vec::new();
    let mut rejected = Vec::new();
    for candidate in preview
        .candidates
        .iter()
        .filter(|candidate| selected.contains(candidate.alias.as_str()))
    {
        if candidate.disposition == AdoptionDisposition::Rejected {
            rejected.push(AdoptionItemResult {
                alias: candidate.alias.clone(),
                disposition: AdoptionDisposition::Rejected,
                detail: candidate.detail.clone(),
            });
            continue;
        }
        let deployments = candidate
            .deployments
            .iter()
            .map(|name| {
                source_deployments
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("source deployment '{name}' disappeared"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        imports.push(RouteImport {
            alias: candidate.alias.clone(),
            primary: candidate.primary.clone(),
            expected_primary: candidate.existing_primary.clone(),
            expected_fallbacks: candidate.existing_fallbacks.clone(),
            fallbacks: candidate.fallbacks.clone(),
            deployments,
        });
    }

    let (routes, merged) =
        inference_routes::import_routes(destination, &imports, replace_alias_conflicts)?;
    let mut items = merged
        .into_iter()
        .map(map_route_result)
        .chain(rejected)
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.alias.cmp(&right.alias));
    let imported = count_disposition(&items, AdoptionDisposition::Imported);
    let unchanged = count_disposition(&items, AdoptionDisposition::Unchanged);
    let conflicting = count_disposition(&items, AdoptionDisposition::Conflicting);
    let rejected = count_disposition(&items, AdoptionDisposition::Rejected);

    Ok(AdoptionResult {
        schema_version: 1,
        source: source_name.to_string(),
        destination: destination.display().to_string(),
        items,
        imported,
        unchanged,
        conflicting,
        rejected,
        routes,
    })
}

fn destination_exists(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err("inference routes must be a regular non-symlink file".to_string())
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot read inference routes metadata: {error}")),
    }
}

fn parse_source(encoded: &str) -> Result<SourceRegistry, String> {
    if encoded.len() > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "configuration input exceeds the {MAX_DOCUMENT_BYTES}-byte limit"
        ));
    }
    if encoded.trim().is_empty() {
        return Ok(SourceRegistry::default());
    }
    serde_json::from_str(encoded)
        .map_err(|error| format!("invalid Brama inference-routes JSON: {error}"))
}

fn validate_source(source: &SourceRegistry) -> Result<(), String> {
    if source.schema_version != schema_version() {
        return Err(format!(
            "unsupported inference-routes schema version {}",
            source.schema_version
        ));
    }
    if source.routes.len() > MAX_ALIASES || source.fallbacks.len() > MAX_ALIASES {
        return Err(format!(
            "inference routes may contain at most {MAX_ALIASES} aliases"
        ));
    }
    if source.deployments.len() > MAX_DEPLOYMENTS {
        return Err(format!(
            "inference routes may contain at most {MAX_DEPLOYMENTS} deployments"
        ));
    }
    let mut deployment_names = HashSet::new();
    for deployment in &source.deployments {
        if !valid_identifier(&deployment.name) || !deployment_names.insert(deployment.name.as_str())
        {
            return Err(format!(
                "deployment names must be valid and unique; rejected '{}'",
                deployment.name
            ));
        }
        if deployment.adapters.len() > 32
            || deployment
                .adapters
                .iter()
                .any(|entry| !valid_identifier(&entry.name))
        {
            return Err(format!(
                "deployment '{}' contains an invalid adapter list",
                deployment.name
            ));
        }
    }
    for (alias, primary) in &source.routes {
        if !valid_alias(alias) {
            return Err(format!("invalid route alias '{alias}'"));
        }
        let fallbacks = source
            .fallbacks
            .get(alias)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut seen = HashSet::from([primary.as_str()]);
        if !valid_destination(primary)
            || fallbacks
                .iter()
                .any(|route| !valid_destination(route) || !seen.insert(route))
        {
            return Err(format!(
                "route chain for '{alias}' is malformed or duplicated"
            ));
        }
    }
    if let Some(alias) = source
        .fallbacks
        .keys()
        .find(|alias| !source.routes.contains_key(*alias))
    {
        return Err(format!(
            "inference fallback route '{alias}' has no primary destination"
        ));
    }
    Ok(())
}

fn validate_request_identity(source_name: &str, agent_id: &str) -> Result<(), String> {
    if source_name.is_empty()
        || source_name.chars().count() > MAX_SOURCE_NAME_CHARACTERS
        || source_name.chars().any(char::is_control)
    {
        return Err("configuration source name is invalid".to_string());
    }
    if agent_id.is_empty()
        || agent_id.chars().count() > MAX_AGENT_ID_CHARACTERS
        || agent_id.bytes().any(|byte| {
            !(byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_' | b'.'))
        })
    {
        return Err("agent id is invalid".to_string());
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.trim() == value
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

fn valid_destination(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_DESTINATION_CHARACTERS
        && value.trim() == value
        && !value.contains('*')
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn source_deployments_by_name(source: &SourceRegistry) -> Result<HashMap<String, Value>, String> {
    source
        .deployments
        .iter()
        .map(|deployment| {
            serde_json::to_value(deployment)
                .map(|value| (deployment.name.clone(), value))
                .map_err(|error| format!("cannot encode deployment '{}': {error}", deployment.name))
        })
        .collect()
}

fn deployments_by_name(document: &Value) -> Result<HashMap<String, Value>, String> {
    document
        .get("deployments")
        .and_then(Value::as_array)
        .ok_or_else(|| "inference routes.deployments must be an array".to_string())?
        .iter()
        .map(|deployment| {
            let name = deployment
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "inference route deployment has no name".to_string())?;
            Ok((name.to_string(), deployment.clone()))
        })
        .collect()
}

fn map_route_result(result: RouteImportResult) -> AdoptionItemResult {
    let disposition = match result.disposition {
        RouteImportDisposition::Imported => AdoptionDisposition::Imported,
        RouteImportDisposition::Unchanged => AdoptionDisposition::Unchanged,
        RouteImportDisposition::Conflicting => AdoptionDisposition::Conflicting,
    };
    AdoptionItemResult {
        alias: result.alias,
        disposition,
        detail: result.detail,
    }
}

fn count_disposition(items: &[AdoptionItemResult], wanted: AdoptionDisposition) -> usize {
    items
        .iter()
        .filter(|item| item.disposition == wanted)
        .count()
}
