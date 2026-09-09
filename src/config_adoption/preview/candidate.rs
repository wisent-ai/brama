//! One alias as the review presents it: the single destination the source
//! names for it, what the registry holds for that alias today, and the reason
//! it can or cannot be adopted.
//!
//! An alias names one destination and is judged on that one destination, so a
//! refusal here is the answer for the alias rather than a note about one leg of
//! a longer route.

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use serde_json::Value;

use crate::core::server::{
    alias_requires_direct_capability, alias_route_shape_supported, BEST_ALIAS,
};
use crate::gateway::broker::SubscriptionEntry;
use crate::providers::adapter;

use super::super::AdoptionDisposition;

#[derive(Debug, Clone, Serialize)]
pub struct AdoptionCandidate {
    pub alias: String,
    pub primary: String,
    pub deployments: Vec<String>,
    pub existing_primary: Option<String>,
    pub disposition: AdoptionDisposition,
    pub detail: String,
}

/// Everything one review judges its candidates against: the identity that
/// asked, what Brama can acquire on its behalf, and the deployments named on
/// each side.
pub(super) struct Review<'a> {
    pub(super) agent_id: &'a str,
    pub(super) configured_providers: &'a HashSet<String>,
    pub(super) subscriptions: &'a Result<Vec<SubscriptionEntry>, String>,
    pub(super) source_deployments: &'a HashMap<String, Value>,
    pub(super) destination_deployments: &'a HashMap<String, Value>,
}

pub(super) fn evaluate(
    review: &Review<'_>,
    alias: &str,
    primary: &str,
    existing_primary: Option<String>,
) -> Result<AdoptionCandidate, String> {
    let names_deployment = primary != BEST_ALIAS && !primary.contains('/');
    let deployments = if names_deployment {
        vec![primary.to_string()]
    } else {
        Vec::new()
    };
    let resolved = if names_deployment {
        format!("local-openai/{primary}")
    } else {
        primary.to_string()
    };

    let (mut disposition, mut detail) = match refusal(review, alias, &resolved)? {
        Some(reason) => (AdoptionDisposition::Rejected, reason),
        None => (
            AdoptionDisposition::Importable,
            "ready to persist through Brama's route registry".to_string(),
        ),
    };

    if disposition != AdoptionDisposition::Rejected {
        if let Some(name) = deployments.iter().find(|name| {
            review
                .source_deployments
                .get(name.as_str())
                .is_some_and(|source_deployment| {
                    review
                        .destination_deployments
                        .get(name.as_str())
                        .is_some_and(|existing| existing != source_deployment)
                })
        }) {
            disposition = AdoptionDisposition::Conflicting;
            detail =
                format!("deployment '{name}' already exists with a different endpoint or adapter");
        } else if existing_primary.is_some() {
            if existing_primary.as_deref() == Some(primary) {
                disposition = AdoptionDisposition::Unchanged;
                detail = "the destination already has this route".to_string();
            } else {
                disposition = AdoptionDisposition::Conflicting;
                detail = "the destination already has a different route".to_string();
            }
        }
    }

    Ok(AdoptionCandidate {
        alias: alias.to_string(),
        primary: primary.to_string(),
        deployments,
        existing_primary,
        disposition,
        detail,
    })
}

/// Why Brama would refuse to serve this alias through this destination, if it
/// would: an unsupported shape for the alias, a delegation to a subscription
/// the agent does not have, or a provider Brama holds no acquisition route for.
fn refusal(review: &Review<'_>, alias: &str, resolved: &str) -> Result<Option<String>, String> {
    if !alias_route_shape_supported(alias, resolved) {
        return Ok(Some(format!(
            "route '{resolved}' is not supported for alias '{alias}'"
        )));
    }
    if resolved == BEST_ALIAS {
        return Ok(match review.subscriptions {
            Ok(entries) if entries.is_empty() => Some(format!(
                "agent '{}' has no discoverable Skarbiec subscription",
                review.agent_id
            )),
            Err(error) => Some(error.clone()),
            Ok(_) => None,
        });
    }
    if alias_requires_direct_capability(alias, resolved) {
        let provider = adapter::provider_id_from_route(resolved)
            .ok_or_else(|| format!("route '{resolved}' has no supported provider"))?;
        if adapter::provider_requires_credential(provider)
            && !review.configured_providers.contains(provider)
        {
            return Ok(Some(format!(
                "provider '{provider}' has no configured Brama acquisition route"
            )));
        }
    }
    Ok(None)
}
