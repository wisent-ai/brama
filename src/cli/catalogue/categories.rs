//! `brama categories`: reading, declaring and retiring the operator's own
//! grouping of the catalogue.
//!
//! Every write goes through the registry's own staged, validated, owner-only
//! path, and every refusal printed here is the registry's own sentence. The
//! model count beside each category is the check on the declaration: a
//! category whose rule matches nothing reads `0` rather than looking like a
//! working facet in two consoles.

use clap::Args;
use serde_json::json;

use brama::core::inference_routes::categories::{
    declared, delete_category, set_category, Category,
};
use brama::core::inference_routes::configured_path;
use brama::subscription_dispatch::model_catalog;

const NO_REGISTRY: &str =
    "no inference route registry is configured; set BRAMA_INFERENCE_ROUTES_FILE";

#[derive(Args)]
pub(crate) struct ListArgs {
    /// Print the declarations as JSON instead of lines
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Args)]
pub(crate) struct SetArgs {
    /// Category name: lowercase letters, digits and hyphens
    name: String,
    /// Every model of this provider is in the category; repeatable
    #[arg(long = "provider")]
    providers: Vec<String>,
    /// This exact provider/model route is in the category; repeatable
    #[arg(long = "route")]
    routes: Vec<String>,
    /// A model whose route or published name contains this text is in the category; repeatable
    #[arg(long = "term")]
    terms: Vec<String>,
}

#[derive(Args)]
pub(crate) struct RemoveArgs {
    /// Category name to retire
    name: String,
}

pub(crate) async fn run_categories(command: super::CatalogueCommand) {
    match command {
        super::CatalogueCommand::Ls(args) => list(args).await,
        super::CatalogueCommand::Set(args) => set(args),
        super::CatalogueCommand::Rm(args) => remove(args),
    }
}

async fn list(args: ListArgs) {
    let declarations = declared();
    let snapshot = model_catalog::snapshot().await;
    if let Err(error) = snapshot.as_ref() {
        eprintln!("the model catalogue could not be read: {error}");
    }
    let counted = declarations
        .iter()
        .map(|(name, category)| {
            let models = snapshot.as_ref().ok().map(|catalog| {
                catalog
                    .models
                    .iter()
                    .filter(|model| category.contains(model))
                    .count()
            });
            (name.clone(), category.clone(), models)
        })
        .collect::<Vec<_>>();
    if args.json {
        super::super::print_json(&json!({
            "registry": configured_path().map(|path| path.display().to_string()),
            "categories": counted
                .iter()
                .map(|(name, category, models)| json!({
                    "category": name,
                    "providers": category.providers,
                    "routes": category.routes,
                    "terms": category.terms,
                    "models": models,
                }))
                .collect::<Vec<_>>(),
        }));
        return;
    }
    if counted.is_empty() {
        println!("no model categories are declared");
        return;
    }
    for (name, category, models) in &counted {
        let models = models.map_or_else(|| "?".to_string(), |count| count.to_string());
        println!("{name} ({models} models)");
        println!("  providers: {}", joined(&category.providers));
        println!("  routes:    {}", joined(&category.routes));
        println!("  terms:     {}", joined(&category.terms));
    }
}

fn set(args: SetArgs) {
    let Some(path) = configured_path() else {
        eprintln!("{NO_REGISTRY}");
        std::process::exit(1);
    };
    let category = Category {
        providers: args.providers,
        routes: args.routes,
        terms: args.terms,
    };
    match set_category(&path, &args.name, &category) {
        Ok(_) => println!("declared model category {}", args.name),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

fn remove(args: RemoveArgs) {
    let Some(path) = configured_path() else {
        eprintln!("{NO_REGISTRY}");
        std::process::exit(1);
    };
    match delete_category(&path, &args.name) {
        Ok(_) => println!("retired model category {}", args.name),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

fn joined(values: &[String]) -> String {
    if values.is_empty() {
        "-".to_string()
    } else {
        values.join(", ")
    }
}
