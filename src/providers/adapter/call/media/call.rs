//! What one vendor media adapter calls the provider with: the route, its
//! declaration and model, and requests already authorized for the provider.

use super::super::super::registry::{endpoint, ProviderDescriptor};
use super::super::credential::authorize_provider;

/// Everything one vendor adapter needs to make its call.
pub(super) struct Call<'a> {
    pub(super) route_id: &'a str,
    pub(super) descriptor: &'static ProviderDescriptor,
    pub(super) model_id: &'a str,
    pub(super) key: &'a str,
    pub(super) base_url: &'a str,
    pub(super) client: &'a reqwest::Client,
    pub(super) secret: &'a str,
}

impl Call<'_> {
    pub(super) fn post(&self, path: &str) -> reqwest::RequestBuilder {
        authorize_provider(
            self.client.post(endpoint(self.base_url, path)),
            self.descriptor,
            self.key,
            self.secret,
        )
    }

    pub(super) fn get(&self, path: &str) -> reqwest::RequestBuilder {
        authorize_provider(
            self.client.get(endpoint(self.base_url, path)),
            self.descriptor,
            self.key,
            self.secret,
        )
    }

    pub(super) fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        authorize_provider(
            self.client.delete(endpoint(self.base_url, path)),
            self.descriptor,
            self.key,
            self.secret,
        )
    }
}
