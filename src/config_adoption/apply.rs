//! Persisting the aliases an operator selected from a review, and reporting
//! per alias what the registry did with each one.

use std::collections::HashSet;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::core::inference_routes::{self, RouteImport, RouteImportDisposition, RouteImportResult};

use super::document::{parse_source, source_deployments_by_name};
use super::preview::preview_document;
use super::{AdoptionDisposition, MAX_ALIASES, SCHEMA_VERSION};

#[derive(Debug, Clone, Serialize)]
pub struct AdoptionItemResult {
    pub alias: String,
    pub disposition: AdoptionDisposition,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AdoptionResult {
    pub schema_version: u32,
    pub source: String,
    pub destination: String,
    pub items: Vec<AdoptionItemResult>,
    pub imported: usize,
    pub unchanged: usize,
    pub conflicting: usize,
    pub rejected: usize,
    pub routes: Value,
}

pub async fn apply_document(
    encoded: &str,
    source_name: &str,
    destination: &Path,
    agent_id: &str,
    selected_aliases: &[String],
    replace_alias_conflicts: bool,
) -> Result<AdoptionResult, String> {
    let preview = preview_document(encoded, source_name, destination, agent_id).await?;
    if selected_aliases.len() > MAX_ALIASES {
        return Err(format!("at most {MAX_ALIASES} aliases may be selected"));
    }
    let selected = selected_aliases
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if selected.len() != selected_aliases.len() {
        return Err("selected aliases must be unique".to_string());
    }
    let known = preview
        .candidates
        .iter()
        .map(|candidate| candidate.alias.as_str())
        .collect::<HashSet<_>>();
    if let Some(alias) = selected.iter().find(|alias| !known.contains(*alias)) {
        return Err(format!(
            "selected alias '{}' is not present in the source",
            alias
        ));
    }

    let source = parse_source(encoded)?;
    let source_deployments = source_deployments_by_name(&source)?;
    let mut imports = Vec::new();
    let mut rejected = Vec::new();
    for candidate in preview
        .candidates
        .iter()
        .filter(|candidate| selected.contains(candidate.alias.as_str()))
    {
        if candidate.disposition == AdoptionDisposition::Rejected {
            rejected.push(AdoptionItemResult {
                alias: candidate.alias.clone(),
                disposition: AdoptionDisposition::Rejected,
                detail: candidate.detail.clone(),
            });
            continue;
        }
        let deployments = candidate
            .deployments
            .iter()
            .map(|name| {
                source_deployments
                    .get(name)
                    .cloned()
                    .ok_or_else(|| format!("source deployment '{name}' disappeared"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        imports.push(RouteImport {
            alias: candidate.alias.clone(),
            primary: candidate.primary.clone(),
            expected_primary: candidate.existing_primary.clone(),
            deployments,
        });
    }

    let (routes, merged) =
        inference_routes::import_routes(destination, &imports, replace_alias_conflicts)?;
    let mut items = merged
        .into_iter()
        .map(map_route_result)
        .chain(rejected)
        .collect::<Vec<_>>();
    items.sort_by(|left, right| left.alias.cmp(&right.alias));
    let imported = count_disposition(&items, AdoptionDisposition::Imported);
    let unchanged = count_disposition(&items, AdoptionDisposition::Unchanged);
    let conflicting = count_disposition(&items, AdoptionDisposition::Conflicting);
    let rejected = count_disposition(&items, AdoptionDisposition::Rejected);

    Ok(AdoptionResult {
        schema_version: SCHEMA_VERSION,
        source: source_name.to_string(),
        destination: destination.display().to_string(),
        items,
        imported,
        unchanged,
        conflicting,
        rejected,
        routes,
    })
}

fn map_route_result(result: RouteImportResult) -> AdoptionItemResult {
    let disposition = match result.disposition {
        RouteImportDisposition::Imported => AdoptionDisposition::Imported,
        RouteImportDisposition::Unchanged => AdoptionDisposition::Unchanged,
        RouteImportDisposition::Conflicting => AdoptionDisposition::Conflicting,
    };
    AdoptionItemResult {
        alias: result.alias,
        disposition,
        detail: result.detail,
    }
}

fn count_disposition(items: &[AdoptionItemResult], wanted: AdoptionDisposition) -> usize {
    items
        .iter()
        .filter(|item| item.disposition == wanted)
        .count()
}
