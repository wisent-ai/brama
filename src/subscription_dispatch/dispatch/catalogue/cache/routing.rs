//! Routing never joins an unbounded provider discovery operation.
//! A background pass publishes models or its actual refusal for the next request.

use std::borrow::Cow;
use std::sync::{LazyLock, Mutex};

use crate::gateway::broker::SubscriptionEntry;
use crate::providers::adapter::{known_max_output_tokens, route, RegistryModel};
use crate::types::{GatewayRefusal, ModelRequest, ModelResponse, Refusal};

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

/// The request a subscription route sends to its provider. A wire that
/// requires an answer length (Anthropic Messages) refuses a request without
/// one; when the caller set none and this build's embedded table does not
/// know the model (one released after the build was cut), the provider
/// adapter would send it unsized and the provider would refuse every request
/// ("max_tokens: Field required"). Such a request is sized here by the limit
/// the provider's own model listing stated
/// in the inventory routing last published, the same discovery the request
/// was routed from. The caller's request goes unchanged when the caller or the
/// table sizes it, or when the listing states no limit for the model. Missing
/// discovery, a failed discovery and unreadable routing state refuse the
/// request with that cause instead of a later wire refusal.
pub(in crate::subscription_dispatch::dispatch) fn sized_request(
    request: &ModelRequest,
) -> Result<Cow<'_, ModelRequest>, ModelResponse> {
    if request.max_tokens.is_some() {
        return Ok(Cow::Borrowed(request));
    }
    let Some((descriptor, model_id)) = route(&request.model) else {
        return Ok(Cow::Borrowed(request));
    };
    let provider_id = descriptor.id;
    let model_id = model_id.as_ref();
    if !descriptor.wire.requires_output_limit()
        || known_max_output_tokens(provider_id, model_id).is_some()
    {
        return Ok(Cow::Borrowed(request));
    }
    let refused = |error: String| {
        ModelResponse::refused(&request.model, GatewayRefusal::DependencyUnavailable, error)
    };
    let Some(limit) = discovered_max_output_tokens(provider_id, model_id).map_err(refused)? else {
        return Ok(Cow::Borrowed(request));
    };
    let max_tokens = u32::try_from(limit).map_err(|error| {
        refused(format!(
            "{provider_id}/{model_id}: the provider's listing states an output limit of {limit} tokens, which a request cannot carry: {error}"
        ))
    })?;
    Ok(Cow::Owned(ModelRequest {
        max_tokens: Some(max_tokens),
        ..request.clone()
    }))
}

fn discovered_max_output_tokens(provider_id: &str, model_id: &str) -> Result<Option<u64>, String> {
    let snapshot = SNAPSHOT.lock().map_err(|error| {
        format!("routing model state is unreadable while sizing {provider_id}/{model_id}: {error}")
    })?;
    let models = match &snapshot.result {
        Some(Ok(models)) => models,
        Some(Err(refusal)) => {
            return Err(format!(
                "cannot size {provider_id}/{model_id}: model discovery failed ({:?}): {}",
                refusal.class, refusal.message
            ));
        }
        None => {
            return Err(format!(
                "cannot size {provider_id}/{model_id}: subscription model discovery has not completed; operation: discover native provider models"
            ));
        }
    };
    Ok(models
        .iter()
        .find(|model| model.provider_id == provider_id && model.model_id == model_id)
        .and_then(|model| model.max_output_tokens))
}
