//! One row of the catalogue listing: the model, the categories it carries,
//! and whether the operator's filters keep it.
//!
//! The filter rules are the gateway's, restated in one place rather than
//! duplicated per flag: an unknown weight is neither open nor closed, and a
//! filter the catalogue cannot answer drops the row instead of guessing.

use serde_json::{json, Value};

use brama::providers::adapter::{ModelKind, RegistryModel};

pub(super) struct ModelRow<'a> {
    pub(super) model: &'a RegistryModel,
    pub(super) categories: Vec<String>,
}

impl ModelRow<'_> {
    /// `unknown` is the third answer and it is not a hedge: models.dev states
    /// `open_weights` per model, and a row that omits it has told us nothing
    /// about how the weights were published.
    pub(super) fn weights(&self) -> &'static str {
        match self.model.open_weights {
            Some(true) => "open",
            Some(false) => "closed",
            None => "unknown",
        }
    }

    pub(super) fn categories_column(&self) -> String {
        if self.categories.is_empty() {
            "-".to_string()
        } else {
            self.categories.join(",")
        }
    }
}

pub(super) fn row_json(row: &ModelRow) -> Value {
    json!({
        "route": row.model.route_id,
        "name": row.model.display_name,
        "provider": row.model.provider_id,
        "kind": row.model.kind().as_str(),
        "openWeights": row.model.open_weights,
        "outputModalities": row.model.output_modalities,
        "inputModalities": row.model.input_modalities,
        "categories": row.categories,
    })
}

pub(super) fn matches_filters(
    row: &ModelRow,
    kind: Option<&str>,
    weights: Option<&str>,
    category: Option<&str>,
    provider: Option<&str>,
    search: Option<&str>,
) -> bool {
    if let Some(kind) = kind.and_then(ModelKind::parse) {
        if row.model.kind() != kind {
            return false;
        }
    }
    if let Some(weights) = weights {
        if row.weights() != weights {
            return false;
        }
    }
    if let Some(category) = category {
        if !row.categories.iter().any(|name| name == category) {
            return false;
        }
    }
    if let Some(provider) = provider {
        if !row.model.provider_id.eq_ignore_ascii_case(provider) {
            return false;
        }
    }
    if let Some(search) = search {
        let needle = search.to_ascii_lowercase();
        let route = row.model.route_id.to_ascii_lowercase();
        let name = row.model.display_name.to_ascii_lowercase();
        if !route.contains(&needle) && !name.contains(&needle) {
            return false;
        }
    }
    true
}
