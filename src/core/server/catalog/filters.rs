//! What a caller may narrow `GET /v1/models` by, and what each answer means.
//!
//! Four facets, each answering one question a caller asks before it picks a
//! name: which endpoint shape answers this model, are the weights published,
//! which of the operator's categories does it carry, and whose model is it.
//! A value this gateway has no meaning for is refused by name rather than
//! returning the whole catalogue — a filter that passes everything through is
//! how a caller ends up posting a chat model to the image endpoint.

use std::collections::HashMap;

use crate::core::inference_routes::categories::valid_category_name;
use crate::core::server::refusal::{api_error, ApiError};
use crate::providers::adapter::{valid_provider_id, ModelKind, RegistryModel};

#[derive(Debug, Default)]
pub(in crate::core::server) struct CatalogFilters {
    kind: Option<String>,
    weights: Option<String>,
    category: Option<String>,
    provider: Option<String>,
}

/// Whether the caller asked for published weights or for withheld ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Weights {
    Open,
    Closed,
}

#[derive(Debug, Default)]
pub(in crate::core::server) struct ValidCatalogFilters {
    kind: Option<ModelKind>,
    weights: Option<Weights>,
    category: Option<String>,
    provider: Option<String>,
}

fn stated(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

impl CatalogFilters {
    /// Read the query members this endpoint narrows on, refusing any other
    /// member by name.
    ///
    /// The query is read as raw pairs rather than deserialized into this
    /// struct, because a deserializer's own rejection is plain text: a
    /// caller that mistyped a member would get a body its error handling
    /// cannot parse, from the one endpoint whose whole job is telling a
    /// caller what it may ask for.
    pub(in crate::core::server) fn from_query(
        query: HashMap<String, String>,
    ) -> Result<Self, ApiError> {
        let mut filters = Self::default();
        for (member, value) in query {
            match member.as_str() {
                "kind" => filters.kind = Some(value),
                "weights" => filters.weights = Some(value),
                "category" => filters.category = Some(value),
                "provider" => filters.provider = Some(value),
                other => {
                    return Err(api_error(
                        axum::http::StatusCode::BAD_REQUEST,
                        &format!(
                            "`{other}` is not a model filter; narrow by kind, weights, category or provider"
                        ),
                    ))
                }
            }
        }
        Ok(filters)
    }
}

impl CatalogFilters {
    pub(in crate::core::server) fn validated(self) -> Result<ValidCatalogFilters, ApiError> {
        let bad_request =
            |message: &'static str| api_error(axum::http::StatusCode::BAD_REQUEST, message);
        let kind = match stated(&self.kind) {
            Some(value) => Some(
                ModelKind::parse(value)
                    .ok_or_else(|| bad_request("kind must be text, image, video or audio"))?,
            ),
            None => None,
        };
        let weights = match stated(&self.weights) {
            Some("open") => Some(Weights::Open),
            Some("closed") => Some(Weights::Closed),
            Some(_) => return Err(bad_request("weights must be open or closed")),
            None => None,
        };
        let category = match stated(&self.category) {
            Some(value) if valid_category_name(value) => Some(value.to_string()),
            Some(_) => {
                return Err(bad_request(
                    "category must be lowercase letters, digits and hyphens",
                ))
            }
            None => None,
        };
        let provider = match stated(&self.provider) {
            Some(value) if valid_provider_id(value) => Some(value.to_ascii_lowercase()),
            Some(_) => return Err(bad_request("provider must be a provider identifier")),
            None => None,
        };
        Ok(ValidCatalogFilters {
            kind,
            weights,
            category,
            provider,
        })
    }
}

impl ValidCatalogFilters {
    pub(in crate::core::server) fn any(&self) -> bool {
        self.kind.is_some()
            || self.weights.is_some()
            || self.category.is_some()
            || self.provider.is_some()
    }

    /// Whether one listed id survives the filter.
    ///
    /// An id the catalogue knows nothing about cannot answer any of these
    /// questions, so it is dropped rather than guessed at: a filtered list
    /// carrying unknowns would be read as "these match", which is the one
    /// thing it must not say. Weights work the same way one level down — a
    /// model whose source never stated them matches neither `open` nor
    /// `closed`.
    pub(in crate::core::server) fn matches(
        &self,
        model: Option<&RegistryModel>,
        categories: &[String],
    ) -> bool {
        if let Some(category) = self.category.as_deref() {
            if !categories.iter().any(|name| name == category) {
                return false;
            }
        }
        let Some(model) = model else {
            return self.kind.is_none() && self.weights.is_none() && self.provider.is_none();
        };
        if let Some(kind) = self.kind {
            if model.kind() != kind {
                return false;
            }
        }
        if let Some(weights) = self.weights {
            let Some(published) = model.open_weights else {
                return false;
            };
            if published != (weights == Weights::Open) {
                return false;
            }
        }
        if let Some(provider) = self.provider.as_deref() {
            if !model.provider_id.eq_ignore_ascii_case(provider) {
                return false;
            }
        }
        true
    }
}
