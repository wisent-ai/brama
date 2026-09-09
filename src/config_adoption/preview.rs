//! What adopting a configuration would do, answered before anything is
//! written: who Brama can act as for the asking agent, which aliases the
//! source names, and what the destination registry holds for each of them.

mod candidate;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::core::inference_routes;
use crate::gateway::broker;

use super::document::{parse_source, source_deployments_by_name, validate_source};
use super::identity::validate_request_identity;
use super::SCHEMA_VERSION;

pub use candidate::AdoptionCandidate;
use candidate::Review;

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
            "schema_version": SCHEMA_VERSION,
            "deployments": [],
            "routes": {},
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
    let destination_deployments = deployments_by_name(&destination_value)?;
    let source_deployments = source_deployments_by_name(&source)?;
    let review = Review {
        agent_id,
        configured_providers: &configured_providers,
        subscriptions: &discovered_subscriptions,
        source_deployments: &source_deployments,
        destination_deployments: &destination_deployments,
    };

    let mut referenced = HashSet::new();
    let mut candidates = Vec::with_capacity(source.routes.len());
    for (alias, primary) in &source.routes {
        let existing_primary = destination_routes
            .get(alias)
            .and_then(Value::as_str)
            .map(str::to_string);
        let candidate = candidate::evaluate(&review, alias, primary, existing_primary)?;
        referenced.extend(candidate.deployments.iter().cloned());
        candidates.push(candidate);
    }

    let mut unreferenced_deployments = source_deployments
        .keys()
        .filter(|name| !referenced.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    unreferenced_deployments.sort();

    Ok(AdoptionPreview {
        schema_version: SCHEMA_VERSION,
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
