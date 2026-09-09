//! The configuration document an operator hands to Brama for adoption: the
//! exact shape it may have, the bounds it is held to before anything reads it,
//! and the deployments it names.
//!
//! Unknown members are refused rather than ignored, so a document written for a
//! shape Brama no longer has is reported to the operator by name instead of
//! being adopted with part of its meaning dropped.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::marker::PhantomData;

use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::core::server::valid_alias;

use super::{
    MAX_ALIASES, MAX_DEPLOYMENTS, MAX_DESTINATION_CHARACTERS, MAX_DOCUMENT_BYTES, SCHEMA_VERSION,
};

fn schema_version() -> u32 {
    SCHEMA_VERSION
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
pub(super) struct SourceRegistry {
    #[serde(default = "schema_version")]
    schema_version: u32,
    #[serde(default)]
    deployments: Vec<SourceDeployment>,
    #[serde(default, deserialize_with = "deserialize_unique_map")]
    pub(super) routes: BTreeMap<String, String>,
}

impl Default for SourceRegistry {
    fn default() -> Self {
        Self {
            schema_version: schema_version(),
            deployments: Vec::new(),
            routes: BTreeMap::new(),
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

pub(super) fn parse_source(encoded: &str) -> Result<SourceRegistry, String> {
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

pub(super) fn validate_source(source: &SourceRegistry) -> Result<(), String> {
    if source.schema_version != schema_version() {
        return Err(format!(
            "unsupported inference-routes schema version {}",
            source.schema_version
        ));
    }
    if source.routes.len() > MAX_ALIASES {
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
        if !valid_destination(primary) {
            return Err(format!("route for '{alias}' is malformed"));
        }
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

pub(super) fn source_deployments_by_name(
    source: &SourceRegistry,
) -> Result<HashMap<String, Value>, String> {
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
