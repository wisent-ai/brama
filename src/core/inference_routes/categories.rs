//! Model categories: the operator's own grouping of the catalogue.
//!
//! A category is a name — `uncensored` is the first one anybody asked for —
//! and the membership rule that decides which models carry it. No public
//! catalogue states categories, so the gateway does not infer them: it reads
//! what the operator declared in the route registry and applies exactly that.
//! A gateway that guessed would be publishing an opinion as metadata, and the
//! caller could not tell the two apart.
//!
//! Membership is declared three ways, and a model matching any of them is in:
//!
//! - `providers` — every model of that provider, for a provider whose whole
//!   catalogue is the category (`abliteration-ai` publishes nothing else);
//! - `routes` — one exact `provider/model` route;
//! - `terms` — a string the publisher put in the model's route or display
//!   name. The terms live in the operator's document rather than in this
//!   binary, so adding `abliterated` is an edit to a file and not a release.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::providers::adapter::RegistryModel;

use super::document::{read, snapshot, write_registry, ROUTE_WRITE_LOCK};

/// A category name is a label in a URL query and a column in two consoles, so
/// it is held to the same shape a route half is: lowercase, short, and free of
/// anything that could split a list.
const MAX_CATEGORY_NAME_BYTES: usize = 32;
const MAX_CATEGORY_MEMBERS: usize = 512;
const MAX_TERM_BYTES: usize = 64;
/// One deployment declares a handful of categories; a document with hundreds
/// is a mistake that would otherwise be paid for on every catalogue read.
const MAX_CATEGORIES: usize = 32;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Category {
    #[serde(default)]
    pub providers: Vec<String>,
    #[serde(default)]
    pub routes: Vec<String>,
    #[serde(default)]
    pub terms: Vec<String>,
}

pub type Categories = BTreeMap<String, Category>;

impl Category {
    /// Whether this model is a member. The match is on what the publisher
    /// named the model, never on its description: a description is prose about
    /// a model and mentions its neighbours, which is how "based on the
    /// uncensored model, with provider moderation for illegal activities"
    /// would otherwise join the category it says it is not in.
    pub fn contains(&self, model: &RegistryModel) -> bool {
        if self
            .providers
            .iter()
            .any(|provider| provider.eq_ignore_ascii_case(&model.provider_id))
        {
            return true;
        }
        if self
            .routes
            .iter()
            .any(|route| route.eq_ignore_ascii_case(&model.route_id))
        {
            return true;
        }
        if self.terms.is_empty() {
            return false;
        }
        let route = model.route_id.to_ascii_lowercase();
        let name = model.display_name.to_ascii_lowercase();
        self.terms
            .iter()
            .any(|term| route.contains(term.as_str()) || name.contains(term.as_str()))
    }

    fn normalized(&self) -> Category {
        Category {
            providers: self
                .providers
                .iter()
                .map(|value| value.trim().to_ascii_lowercase())
                .collect(),
            routes: self
                .routes
                .iter()
                .map(|value| value.trim().to_string())
                .collect(),
            terms: self
                .terms
                .iter()
                .map(|value| value.trim().to_ascii_lowercase())
                .collect(),
        }
    }

    fn validate(&self, name: &str) -> Result<(), String> {
        let members = self.providers.len() + self.routes.len() + self.terms.len();
        if members == usize::MIN {
            return Err(format!(
                "model category '{name}' declares no providers, routes or terms"
            ));
        }
        if members > MAX_CATEGORY_MEMBERS {
            return Err(format!(
                "model category '{name}' declares more than {MAX_CATEGORY_MEMBERS} members"
            ));
        }
        for provider in &self.providers {
            if !crate::providers::adapter::valid_provider_id(provider) {
                return Err(format!(
                    "model category '{name}' names an invalid provider '{provider}'"
                ));
            }
        }
        for route in &self.routes {
            if crate::providers::adapter::provider_id_from_route(route).is_none() {
                return Err(format!(
                    "model category '{name}' names '{route}', which is not a provider/model route"
                ));
            }
        }
        for term in &self.terms {
            if term.is_empty() || term.len() > MAX_TERM_BYTES {
                return Err(format!(
                    "model category '{name}' declares a term that is empty or longer than {MAX_TERM_BYTES} bytes"
                ));
            }
        }
        Ok(())
    }
}

/// A category name as it is written in the document and in `?category=`.
pub fn valid_category_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CATEGORY_NAME_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Validate the `categories` member of one registry document.
pub fn validate_categories(categories: &Categories) -> Result<(), String> {
    if categories.len() > MAX_CATEGORIES {
        return Err(format!(
            "inference routes declare more than {MAX_CATEGORIES} model categories"
        ));
    }
    for (name, category) in categories {
        if !valid_category_name(name) {
            return Err(format!(
                "model category '{name}' must be lowercase letters, digits and hyphens, at most {MAX_CATEGORY_NAME_BYTES} bytes"
            ));
        }
        category.normalized().validate(name)?;
    }
    Ok(())
}

/// Every category the operator declared, or an empty map where no registry
/// file is configured. A deployment without the file declares no categories;
/// that is not an error, and the catalogue answers accordingly.
pub fn declared() -> Categories {
    let Some(path) = super::configured_path() else {
        return Categories::new();
    };
    match read(&path) {
        Ok(registry) => registry
            .categories
            .iter()
            .map(|(name, category)| (name.clone(), category.normalized()))
            .collect(),
        Err(error) => {
            tracing::warn!(%error, "model categories unavailable");
            Categories::new()
        }
    }
}

/// The categories one model belongs to, sorted, so two callers reading the
/// same catalogue read the same order.
pub fn categories_for(categories: &Categories, model: &RegistryModel) -> Vec<String> {
    let mut names = categories
        .iter()
        .filter(|(_, category)| category.contains(model))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

/// The names declared, for a console that lists them before anything is
/// filtered.
pub fn declared_names(categories: &Categories) -> Vec<String> {
    categories
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Declare or replace one category, through the same staged, validated,
/// owner-only write every other registry edit takes.
pub fn set_category(path: &Path, name: &str, category: &Category) -> Result<Value, String> {
    if !valid_category_name(name) {
        return Err(format!(
            "model category '{name}' must be lowercase letters, digits and hyphens, at most {MAX_CATEGORY_NAME_BYTES} bytes"
        ));
    }
    let normalized = category.normalized();
    normalized.validate(name)?;
    let _guard = ROUTE_WRITE_LOCK
        .lock()
        .map_err(|_| "inference route write lock is poisoned".to_string())?;
    let mut value = snapshot(path)?;
    let document = value
        .as_object_mut()
        .ok_or_else(|| "inference routes must be a JSON object".to_string())?;
    let declared = document
        .entry("categories")
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object_mut()
        .ok_or_else(|| "inference routes.categories must be an object".to_string())?;
    declared.insert(
        name.to_string(),
        serde_json::to_value(&normalized)
            .map_err(|error| format!("cannot encode model category: {error}"))?,
    );
    write_registry(path, &value)?;
    Ok(value)
}

/// Retire one category. Removing a name nobody declared is a refusal rather
/// than a silent success, because the operator is otherwise told the document
/// changed when it did not.
pub fn delete_category(path: &Path, name: &str) -> Result<Value, String> {
    let _guard = ROUTE_WRITE_LOCK
        .lock()
        .map_err(|_| "inference route write lock is poisoned".to_string())?;
    let mut value = snapshot(path)?;
    let document = value
        .as_object_mut()
        .ok_or_else(|| "inference routes must be a JSON object".to_string())?;
    let removed = document
        .get_mut("categories")
        .and_then(Value::as_object_mut)
        .and_then(|declared| declared.remove(name));
    if removed.is_none() {
        return Err(format!("no model category '{name}' is declared"));
    }
    write_registry(path, &value)?;
    Ok(value)
}
