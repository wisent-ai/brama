//! `brama models` and `brama categories`: what this gateway can name, and how
//! the operator groups it.
//!
//! Both read the public catalogue and the operator's route registry directly,
//! without a gateway bearer, because both are readable from an operator shell
//! and the question "which image models are there, and which of them are in
//! my uncensored category" is asked before a gateway is running as often as
//! after.

mod categories;
mod rows;

use clap::{Args, Subcommand};
use serde_json::json;

use brama::core::inference_routes::categories::{categories_for, declared};
use brama::providers::adapter::{ModelKind, Weights};
use brama::subscription_dispatch::model_catalog;

pub(crate) use categories::run_categories;

use rows::{matches_filters, row_json, ModelRow};

#[derive(Args)]
pub(crate) struct ModelsArgs {
    /// Only models of this kind: text, image, video or audio
    #[arg(long)]
    kind: Option<String>,
    /// Only models whose weights are published (open) or withheld (closed)
    #[arg(long)]
    weights: Option<String>,
    /// Only models in this declared category, such as uncensored
    #[arg(long)]
    category: Option<String>,
    /// Only models of this provider
    #[arg(long)]
    provider: Option<String>,
    /// Only models whose route or name contains this text
    #[arg(long)]
    search: Option<String>,
    /// How many matches to print; every one when not given
    #[arg(long)]
    limit: Option<usize>,
    /// Print the listing as JSON instead of a table
    #[arg(long, default_value_t = false)]
    json: bool,
}

pub(crate) async fn models(args: ModelsArgs) {
    let ModelsArgs {
        kind,
        weights,
        category,
        provider,
        search,
        limit,
        json: as_json,
    } = args;
    // The gateway refuses an unreadable filter by name on `GET /v1/models`;
    // this refuses it in the same words, so an operator who learns one
    // surface has learned the other.
    if let Some(kind) = kind.as_deref() {
        if ModelKind::parse(kind).is_none() {
            crate::cli::refused("kind must be text, image, video or audio");
        }
    }
    let weights = match weights.as_deref() {
        Some(value) => match Weights::parse(value) {
            Some(weights) => Some(weights),
            None => crate::cli::refused("weights must be open or closed"),
        },
        None => None,
    };
    let snapshot = match model_catalog::snapshot().await {
        Ok(snapshot) => snapshot,
        Err(error) => crate::cli::refused(format!("the model catalogue could not be read: {error}")),
    };
    let source = model_catalog::source().await;
    let declared_categories = declared();
    let matched = snapshot
        .models
        .iter()
        .map(|model| ModelRow {
            model,
            categories: categories_for(&declared_categories, model),
        })
        .filter(|row| {
            matches_filters(
                row,
                kind.as_deref(),
                weights,
                category.as_deref(),
                provider.as_deref(),
                search.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    let shown = match limit {
        Some(limit) => matched.len().min(limit),
        None => matched.len(),
    };
    if as_json {
        super::print_json(&json!({
            "catalogRevision": snapshot.revision,
            "catalogSource": source,
            "matched": matched.len(),
            "models": matched
                .iter()
                .take(shown)
                .map(row_json)
                .collect::<Vec<_>>(),
        }));
        return;
    }
    println!(
        "{:<52} {:<6} {:<8} {:<16} PROVIDER",
        "ROUTE", "KIND", "WEIGHTS", "CATEGORIES"
    );
    for row in matched.iter().take(shown) {
        println!(
            "{:<52} {:<6} {:<8} {:<16} {}",
            row.model.route_id,
            row.model.kind().as_str(),
            row.weights(),
            row.categories_column(),
            row.model.provider_id
        );
    }
    println!(
        "{shown} of {} matching models, catalogue {}",
        matched.len(),
        snapshot.revision
    );
    match source {
        Some(model_catalog::CatalogSource::Live) => {
            println!("catalogue read live from its source on this load")
        }
        Some(model_catalog::CatalogSource::LastFetchedCopy {
            written_at,
            live_error,
        }) => println!(
            "catalogue is the copy written {written_at}; the live source could not be read: {live_error}"
        ),
        None => println!("catalogue source was not recorded for this load"),
    }
}

#[derive(Subcommand)]
pub(crate) enum CatalogueCommand {
    /// List every declared model category and how many models it holds
    Ls(categories::ListArgs),
    /// Declare or replace one category
    Set(categories::SetArgs),
    /// Retire one category
    Rm(categories::RemoveArgs),
}
