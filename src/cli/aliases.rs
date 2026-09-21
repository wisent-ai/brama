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
    /// Declare where one alias points, in this gateway's route registry
    Set {
        /// The alias a caller names, such as `decision-model`
        alias: String,
        /// The destination it resolves to: a `provider/model` route, a
        /// deployment name, or `best` to delegate to subscription dispatch
        destination: String,
        /// Route registry to write; defaults to BRAMA_INFERENCE_ROUTES_FILE or ~/.config/brama/inference-routes.json
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
        /// Print the committed registry as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Remove one alias from this gateway's route registry
    Rm {
        /// The alias to remove
        alias: String,
        /// Route registry to write; defaults to BRAMA_INFERENCE_ROUTES_FILE or ~/.config/brama/inference-routes.json
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
        /// Print the committed registry as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

/// What the pool holds, for the aliases this gateway cannot check on its own.
///
/// `serving` for a subscription selector means the selector is declared. It
/// says nothing about a credential, and on 2026-09-21 that gap cost an
/// evening: `best serving best` was printed while every request for `best`
/// was refused with `subscription_reauthorization_required`, because the pool
/// held no live member. The pool's own count now stands beside it.
struct PoolCount {
    live: usize,
    members: usize,
}

async fn pool_count() -> PoolCount {
    let scope = brama::subscription_dispatch::pool::PoolScope::Deployment;
    let report = brama::subscription_dispatch::pool::report(&scope).await;
    let rows: &[Value] = report
        .get("subscriptions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    PoolCount {
        live: rows
            .iter()
            .filter(|row| row.get("state").and_then(Value::as_str) == Some("live"))
            .count(),
        members: rows.len(),
    }
}

pub(crate) async fn report(args: AliasesArgs) {
    let AliasesArgs { json, strict } = args;
    let report = match brama::core::server::alias_report() {
        Ok(report) => report,
        Err(error) => {
            eprintln!("aliases could not be read: {error}");
            std::process::exit(1);
        }
    };
    let unserviceable = report.unserviceable();
    let selectors = report
        .aliases
        .iter()
        .filter(|alias| alias.subscription_resolved)
        .count();
    let pool = if selectors > 0 {
        Some(pool_count().await)
    } else {
        None
    };
    let pool_is_dry = pool.as_ref().is_some_and(|pool| pool.live == 0);
    if json {
        super::print_json(&serde_json::json!({
            "source": report.source,
            "aliases": report.aliases,
            "unserviceable": unserviceable,
            "subscription_pool": pool.as_ref().map(|pool| serde_json::json!({
                "live": pool.live,
                "members": pool.members,
                "selectors": selectors,
            })),
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
        if let Some(pool) = &pool {
            println!(
                "subscription pool: {} of {} credential(s) are live, which is what the {} \
                 selector alias(es) above resolve through",
                pool.live, pool.members, selectors
            );
            if pool.live == 0 {
                println!(
                    "every request for a selector alias is refused with \
                     subscription_reauthorization_required until one account is signed in: \
                     `brama subscription sign-in` through Weles, or \
                     `brama subscription sign-in-manual` in your own browser. \
                     `brama subscriptions` says why each member is not live."
                );
            }
        }
    }
    if strict && (unserviceable > 0 || pool_is_dry) {
        std::process::exit(1);
    }
}

pub(crate) fn routes(command: RoutesCommand) {
    match command {
        RoutesCommand::Migrate { file, json } => migrate(file, json),
        RoutesCommand::Set {
            alias,
            destination,
            file,
            json,
        } => set(alias, destination, file, json),
        RoutesCommand::Rm { alias, file, json } => remove(alias, file, json),
    }
}

/// The registry file this command acts on.
fn registry_path(file: Option<PathBuf>) -> PathBuf {
    match file {
        Some(path) => path,
        None => match brama::config_adoption::default_destination() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("route registry error: {error}");
                std::process::exit(1);
            }
        },
    }
}

/// Declare one alias, through the same validated atomic write
/// `PUT /v1/admin/routes` uses.
///
/// The shape is checked here — an alias that promises a typed answer cannot
/// be pointed at a route that cannot produce one — and whether the route can
/// be served on this host is what `brama aliases` reports afterwards. An
/// operator shell holds no provider capability, so refusing the write for a
/// credential this process cannot see would make the registry unwritable
/// from the one place a gateway that is not running can be repaired.
fn set(alias: String, destination: String, file: Option<PathBuf>, json: bool) {
    if !brama::core::server::valid_alias(&alias) {
        eprintln!(
            "route alias `{alias}` is invalid: an alias is lowercase letters, digits, `-`, `_`, `.` and `/`"
        );
        std::process::exit(1);
    }
    if !brama::core::server::route_shape_writable(&alias, &destination) {
        eprintln!(
            "alias `{alias}` cannot carry `{destination}`: it is not a shape this alias promises"
        );
        std::process::exit(1);
    }
    let path = registry_path(file);
    let document = match brama::core::inference_routes::update_route(&path, &alias, &destination) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("route update error: {error}");
            std::process::exit(1);
        }
    };
    report_registry(&path, &document, json, &format!("{alias} -> {destination}"));
}

fn remove(alias: String, file: Option<PathBuf>, json: bool) {
    if !brama::core::server::valid_alias(&alias) {
        eprintln!("route alias `{alias}` is invalid");
        std::process::exit(1);
    }
    let path = registry_path(file);
    let document = match brama::core::inference_routes::delete_route(&path, &alias) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("route removal error: {error}");
            std::process::exit(1);
        }
    };
    report_registry(&path, &document, json, &format!("{alias} removed"));
}

fn report_registry(path: &std::path::Path, document: &Value, json: bool, change: &str) {
    if json {
        super::print_json(&serde_json::json!({
            "registry": path.display().to_string(),
            "change": change,
            "document": document,
        }));
        return;
    }
    println!("registry: {}", path.display());
    println!("change: {change}");
    let routes = document.get("routes").and_then(Value::as_object);
    println!("routes: {}", routes.map_or(0, serde_json::Map::len));
    for (alias, destination) in routes.into_iter().flatten() {
        println!("{:<32} {}", alias, destination.as_str().unwrap_or("-"));
    }
}

/// Move one operator's registry onto the current document shape, in place and
/// through the same validated atomic write every other route change uses.
fn migrate(file: Option<PathBuf>, json: bool) {
    let path = registry_path(file);
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
