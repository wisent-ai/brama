//! `brama launcher …`: what `start-with-skarbiec` needs computed before the
//! gateway starts, done by the binary it is about to start rather than by an
//! interpreter the host may not have.

mod catalog;
mod identities;
mod policy;
mod verbs;

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
    /// The preloaded model-router client table (JSON) for BRAMA_ALLOWED_MODELS
    ModelRouterIdentities {
        #[arg(long)]
        router: PathBuf,
        /// JSON list of the backend client's exact aliases
        #[arg(long)]
        backend_models: String,
    },
    /// Every product's request-sign identity (JSON object)
    RequestSignIdentities {
        #[arg(long)]
        router: PathBuf,
    },
    /// One non-empty field of one vault item, printed bare
    ItemField {
        #[arg(long)]
        router: PathBuf,
        item: String,
        field: String,
    },
    /// Check that the pinned broker advertises every router command path the
    /// launcher and these launcher steps invoke
    CheckRouterVerbs {
        #[arg(long)]
        router: PathBuf,
        #[arg(long)]
        launcher: PathBuf,
    },
}

fn printed(result: Result<String, String>) -> Result<(), String> {
    result.map(|text| print!("{text}"))
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
        LauncherCommand::ModelRouterIdentities { router, backend_models } => std::env::var("BRAMA_ALLOWED_MODELS")
            .map_err(|_| "BRAMA_ALLOWED_MODELS is required".to_string())
            .and_then(|allowed| printed(identities::model_router(&router, &allowed, &backend_models))),
        LauncherCommand::RequestSignIdentities { router } => printed(identities::request_sign(&router)),
        LauncherCommand::ItemField { router, item, field } => printed(identities::item_field(&router, &item, &field)),
        LauncherCommand::CheckRouterVerbs { router, launcher } => verbs::check(&router, &launcher),
    };
    if let Err(detail) = result {
        eprintln!("{detail}");
        std::process::exit(1);
    }
}
