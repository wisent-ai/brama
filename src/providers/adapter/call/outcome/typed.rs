//! The refusals every typed call (embeddings, moderations, decisions, images,
//! video, speech) shares before the provider answers, each with its class
//! stated where it is made.

use serde_json::Value;

use super::super::super::registry::{provider_base_url, route, ProviderDescriptor};
use super::super::credential::provider_credential_key;
use super::super::dispatch_client;
use crate::types::{GatewayRefusal, ProviderRefusal, Refusal};

/// The descriptor and provider model id of a canonical route, refused as a
/// request for a route that does not exist here.
pub(in crate::providers::adapter) fn typed_route(
    route_id: &str,
) -> Result<(&'static ProviderDescriptor, String), Refusal> {
    route(route_id)
        .map(|(descriptor, model_id)| (descriptor, model_id.to_string()))
        .ok_or_else(|| {
            Refusal::gateway(
                GatewayRefusal::InvalidRequest,
                "invalid provider/model route",
            )
        })
}

/// The key, origin and client one typed call is sent with.
///
/// A credential that carries no key is an authorization chain only an
/// operator repairs; an origin the trusted-host policy refuses is this
/// deployment's configuration; a client that cannot be built is a dependency.
pub(in crate::providers::adapter) fn typed_transport(
    descriptor: &ProviderDescriptor,
    item: &str,
    secret: &str,
) -> Result<(String, String, reqwest::Client), Refusal> {
    let key = provider_credential_key(descriptor, item, secret)
        .map_err(|error| Refusal::gateway(GatewayRefusal::CredentialUnauthorized, error))?;
    let base_url = provider_base_url(descriptor)
        .map_err(|error| Refusal::gateway(GatewayRefusal::ProviderFailure, error))?;
    let client = dispatch_client().map_err(|_| {
        Refusal::gateway(
            GatewayRefusal::DependencyUnavailable,
            "dependency_unavailable: provider client could not be built",
        )
    })?;
    Ok((key, base_url, client))
}

/// A provider answer that is not the JSON object every typed call returns.
pub(in crate::providers::adapter) fn typed_object(text: &str) -> Result<Value, Refusal> {
    let body: Value = serde_json::from_str(text).map_err(|_| {
        Refusal::new(
            ProviderRefusal::ProviderFailure,
            "provider_failure: provider returned malformed JSON",
        )
    })?;
    if !body.is_object() {
        return Err(Refusal::new(
            ProviderRefusal::ProviderFailure,
            "provider_failure: provider returned a non-object response",
        ));
    }
    Ok(body)
}
