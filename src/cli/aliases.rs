//! `brama aliases` and `brama routes`: what this gateway declares it can
//! serve, and the route registry those declarations are read from.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use serde_json::Value;

#[derive(Args)]
pub(crate) struct AliasesArgs {
    /// Print the report as JSON instead of lines
    #[arg(long, default_value_t = false)]
    json: bool,
    /// Exit non-zero when any declared alias cannot be served
    #[arg(long, default_value_t = false)]
    strict: bool,
}

#[derive(Subcommand)]
pub(crate) enum RoutesCommand {
    /// Rewrite the route registry into the current document shape
    Migrate {
        /// Route registry to migrate; defaults to BRAMA_INFERENCE_ROUTES_FILE or ~/.config/brama/inference-routes.json
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
        /// Print the migrated registry as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

pub(crate) fn report(args: AliasesArgs) {
    let AliasesArgs { json, strict } = args;
    let report = match brama::core::server::alias_report() {
        Ok(report) => report,
        Err(error) => {
            eprintln!("aliases could not be read: {error}");
            std::process::exit(1);
        }
    };
    let unserviceable = report.unserviceable();
    if json {
        super::print_json(&serde_json::json!({
            "source": report.source,
            "aliases": report.aliases,
            "unserviceable": unserviceable,
        }));
    } else {
        match &report.source.routes_file {
            Some(path) => println!("route registry: {}", path.display()),
            None => println!("route registry: none configured"),
        }
        println!(
            "launcher alias table: {}",
            if report.source.launcher_table_present {
                "present in this process"
            } else {
                "absent; only the compiled-in contract and the route registry are visible here"
            }
        );
        for alias in &report.aliases {
            println!(
                "{:<32} {:<20} {}",
                alias.alias,
                alias.state,
                alias.route.as_deref().unwrap_or("-")
            );
            if let Some(reason) = &alias.reason {
                println!("{:<32} {}", "", reason);
            }
        }
        println!(
            "{} alias(es), {} cannot be served",
            report.aliases.len(),
            unserviceable
        );
    }
    if strict && unserviceable > 0 {
        std::process::exit(1);
    }
}

pub(crate) fn routes(command: RoutesCommand) {
    match command {
        RoutesCommand::Migrate { file, json } => migrate(file, json),
    }
}

/// Move one operator's registry onto the current document shape, in place and
/// through the same validated atomic write every other route change uses.
fn migrate(file: Option<PathBuf>, json: bool) {
    let path = match file {
        Some(path) => path,
        None => match brama::config_adoption::default_destination() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("route migration error: {error}");
                std::process::exit(1);
            }
        },
    };
    let document = match brama::core::inference_routes::migrate(&path) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("route migration error: {error}");
            std::process::exit(1);
        }
    };
    if json {
        super::print_json(&serde_json::json!({
            "registry": path.display().to_string(),
            "document": document,
        }));
        return;
    }
    println!("registry: {}", path.display());
    println!(
        "deployments: {}",
        document
            .get("deployments")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    );
    let routes = document.get("routes").and_then(Value::as_object);
    println!("routes: {}", routes.map_or(0, serde_json::Map::len));
    for (alias, destination) in routes.into_iter().flatten() {
        println!("{:<32} {}", alias, destination.as_str().unwrap_or("-"));
    }
}
