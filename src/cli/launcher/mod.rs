//! `brama launcher …`: what `start-with-skarbiec` needs computed before the
//! gateway starts, done by the binary it is about to start rather than by an
//! interpreter the host may not have.

mod catalog;
mod policy;

use std::path::PathBuf;

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum LauncherCommand {
    /// Build the runtime subscription catalog from the vault's item list and
    /// the runtime policy
    Catalog {
        /// `skarbiec-entitlements-router list` output
        #[arg(long)]
        available: PathBuf,
        #[arg(long)]
        policy: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Validate `services.brama` of the control document and write the
    /// allowed models, the alias routes and the backend aliases, each to a
    /// new owner-only file
    Policy {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        allowed: PathBuf,
        #[arg(long)]
        aliases: PathBuf,
        #[arg(long)]
        backend: PathBuf,
    },
    /// Create the first inference-route registry at PATH from
    /// BRAMA_MODEL_ALIASES
    SeedRoutes { path: PathBuf },
}

pub(crate) fn run(command: LauncherCommand) {
    let result = match command {
        LauncherCommand::Catalog { available, policy, output } => catalog::build(&available, &policy, &output),
        LauncherCommand::Policy { config, allowed, aliases, backend } => policy::check(
            &config,
            &policy::Outputs { allowed: &allowed, aliases: &aliases, backend: &backend },
        ),
        LauncherCommand::SeedRoutes { path } => std::env::var("BRAMA_MODEL_ALIASES")
            .map_err(|_| "BRAMA_MODEL_ALIASES is required".to_string())
            .and_then(|aliases| policy::seed_routes(&path, &aliases)),
    };
    if let Err(detail) = result {
        eprintln!("{detail}");
        std::process::exit(1);
    }
}
