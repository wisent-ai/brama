//! `brama aliases` and `brama routes`: what this gateway declares it can
//! serve, and the route registry those declarations are read from, here or
//! on the gateway that serves them.

mod pool;
mod registry;
mod remote;

use std::path::PathBuf;

use clap::{Args, Subcommand};

use pool::pool_count;

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
    /// Declare where one alias points, in this gateway's route registry or on the gateway named
    Set {
        /// The alias a caller names, such as `decision-model`
        alias: String,
        /// The destination it resolves to: a `provider/model` route, a
        /// deployment name, or `best` to delegate to subscription dispatch
        destination: String,
        /// Route registry to write; defaults to BRAMA_INFERENCE_ROUTES_FILE or ~/.config/brama/inference-routes.json
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
        #[command(flatten)]
        gateway: remote::GatewayArgs,
        /// Print the committed registry as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Remove one alias from this gateway's route registry or from the gateway named
    Rm {
        /// The alias to remove
        alias: String,
        /// Route registry to write; defaults to BRAMA_INFERENCE_ROUTES_FILE or ~/.config/brama/inference-routes.json
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
        #[command(flatten)]
        gateway: remote::GatewayArgs,
        /// Print the committed registry as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

pub(crate) async fn report(args: AliasesArgs) {
    let AliasesArgs { json, strict } = args;
    let report = match brama::core::server::alias_report() {
        Ok(report) => report,
        Err(error) => {
            eprintln!("aliases could not be read: {error}");
            crate::cli::fail();
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
            "last_answers": report.last_answers,
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
            let route = match alias.route.as_deref() {
                Some(route) => route,
                None => "(no route)",
            };
            println!("{:<32} {:<20} {route}", alias.alias, alias.state);
            if let Some(reason) = &alias.reason {
                println!("{:<32} {}", "", reason);
            }
            if let Some(last) = report.last_answers.get(&alias.alias) {
                match (&last.refusal, &last.message) {
                    (Some(code), Some(message)) => {
                        println!(
                            "    last answer since {}: refused {code}: {message}",
                            last.at
                        )
                    }
                    (Some(code), None) => {
                        println!("    last answer since {}: refused {code}", last.at)
                    }
                    (None, _) => println!("    last answer since {}: answered", last.at),
                }
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
                     subscription_reauthorization_required until one account is signed in. The \
                     gateway's renewal sweep signs accounts in through Weles on its own; \
                     `brama subscription list` names what blocks each member under \
                     sign_in_blocked, and a Google prompt the sign-in raises is approved on the \
                     account owner's phone."
                );
            }
        }
    }
    if strict && (unserviceable > 0 || pool_is_dry) {
        crate::cli::fail();
    }
}

pub(crate) async fn routes(command: RoutesCommand) {
    let applied = match command {
        RoutesCommand::Migrate { file, json } => return registry::migrate(file, json),
        RoutesCommand::Set {
            alias,
            destination,
            file,
            gateway,
            json,
        } => match gateway.destination() {
            Some(target) => {
                remote::apply(target, remote::Change::Set { alias, destination }, json).await
            }
            None => return registry::set(alias, destination, file, json),
        },
        RoutesCommand::Rm {
            alias,
            file,
            gateway,
            json,
        } => match gateway.destination() {
            Some(target) => remote::apply(target, remote::Change::Remove { alias }, json).await,
            None => return registry::remove(alias, file, json),
        },
    };
    if let Err(error) = applied {
        eprintln!("{error}");
        crate::cli::fail();
    }
}
