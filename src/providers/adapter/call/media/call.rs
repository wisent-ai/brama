//! What one vendor media adapter calls the provider with: the route, its
//! declaration and model, and requests already authorized for the provider.

use super::super::super::registry::{endpoint, ProviderDescriptor, WireProtocol};
use super::super::credential::{authorize_with_account, credential_account_id};
use crate::types::{GatewayRefusal, Refusal};

/// Everything one vendor adapter needs to make its call.
pub(super) struct Call<'a> {
    pub(super) route_id: &'a str,
    pub(super) descriptor: &'static ProviderDescriptor,
    pub(super) model_id: &'a str,
    pub(super) key: &'a str,
    pub(super) base_url: &'a str,
    pub(super) client: &'a reqwest::Client,
    account_id: Option<String>,
}

impl<'a> Call<'a> {
    pub(super) fn new(
        route_id: &'a str,
        descriptor: &'static ProviderDescriptor,
        model_id: &'a str,
        key: &'a str,
        base_url: &'a str,
        client: &'a reqwest::Client,
        secret: &str,
    ) -> Result<Self, Refusal> {
        let account_id = if descriptor.wire == WireProtocol::OpenAiResponses {
            credential_account_id(secret)
                .map_err(|error| Refusal::gateway(GatewayRefusal::CredentialUnauthorized, error))?
        } else {
            None
        };
        Ok(Self {
            route_id,
            descriptor,
            model_id,
            key,
            base_url,
            client,
            account_id,
        })
    }

    pub(super) fn post(&self, path: &str) -> reqwest::RequestBuilder {
        authorize_with_account(
            self.client.post(endpoint(self.base_url, path)),
            self.descriptor,
            self.key,
            self.account_id.as_deref(),
        )
    }

    pub(super) fn get(&self, path: &str) -> reqwest::RequestBuilder {
        authorize_with_account(
            self.client.get(endpoint(self.base_url, path)),
            self.descriptor,
            self.key,
            self.account_id.as_deref(),
        )
    }

    pub(super) fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        authorize_with_account(
            self.client.delete(endpoint(self.base_url, path)),
            self.descriptor,
            self.key,
            self.account_id.as_deref(),
        )
    }

    /// Send one already-authorized media operation with the product's wait
    /// receipt, including the provider URL and its eventual refusal.
    pub(super) async fn send(
        &self,
        builder: reqwest::RequestBuilder,
        operation: &str,
    ) -> Result<reqwest::Response, Refusal> {
        let (client, request) = builder.build_split();
        let request =
            request.map_err(|error| super::super::outcome::refusal::transport_refusal(&error))?;
        stado_wait::send_built(stado_wait::Kind::Network, operation, client, request)
            .await
            .map_err(|error| super::super::outcome::refusal::transport_refusal(&error))
    }
}
