//! Routing never joins an unbounded provider discovery operation.
//! A background pass publishes models or its actual refusal for the next request.

use std::sync::{LazyLock, Mutex};

use crate::gateway::broker::SubscriptionEntry;
use crate::providers::adapter::RegistryModel;
use crate::types::{GatewayRefusal, Refusal};

use super::super::subscription_models::discover_subscription_models;

type DiscoveryResult = Result<Vec<RegistryModel>, Refusal>;

#[derive(Default)]
struct Snapshot {
    result: Option<DiscoveryResult>,
    refreshing: bool,
}

static SNAPSHOT: LazyLock<Mutex<Snapshot>> = LazyLock::new(|| Mutex::new(Snapshot::default()));

pub(in crate::subscription_dispatch::dispatch::catalogue) fn models(
    entries: Vec<SubscriptionEntry>,
) -> DiscoveryResult {
    if entries.is_empty() {
        return Err(Refusal::gateway(
            GatewayRefusal::SubscriptionUnavailable,
            "the subscription inventory contains no active member",
        ));
    }
    let mut snapshot = SNAPSHOT.lock().map_err(|error| {
        Refusal::gateway(
            GatewayRefusal::DependencyUnavailable,
            format!("model discovery state is unavailable: {error}"),
        )
    })?;
    let result = match &snapshot.result {
        Some(result) => result.clone(),
        None => Err(Refusal::gateway(
            GatewayRefusal::DependencyUnavailable,
            "subscription model discovery has not completed; operation: discover native provider models",
        )),
    };
    if !snapshot.refreshing {
        snapshot.refreshing = true;
        tokio::spawn(async move {
            let _ = refresh(entries).await;
        });
    }
    result
}

pub(in crate::subscription_dispatch::dispatch::catalogue) async fn refresh(
    entries: Vec<SubscriptionEntry>,
) -> DiscoveryResult {
    let result = discover_subscription_models(entries).await;
    if let Err(refusal) = &result {
        tracing::warn!(event = "routing_model_discovery_failed", error = %refusal.message, "routing refuses without waiting for a provider");
    }
    let mut snapshot = SNAPSHOT.lock().map_err(|error| {
        Refusal::gateway(
            GatewayRefusal::DependencyUnavailable,
            format!("routing model state could not publish discovery: {error}"),
        )
    })?;
    snapshot.result = Some(result.clone());
    snapshot.refreshing = false;
    result
}
