//! `brama launcher …`: what `start-with-skarbiec` needs computed before the
//! gateway starts, done by the binary it is about to start rather than by an
//! interpreter the host may not have.

mod catalog;
pub(crate) mod identities;
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
        /// Access policy JSON whose `roles.brama-runtime` rules name the
        /// purposes and resources the gateway may read
        #[arg(long)]
        policy: PathBuf,
        /// File the catalog is written to
        #[arg(long)]
        output: PathBuf,
    },
    /// Validate `services.brama` of the control document and write the
    /// allowed models, alias routes, backend aliases and capability lifetime,
    /// each to a new owner-only file
    Policy {
        /// The control document holding `services.brama`
        #[arg(long)]
        config: PathBuf,
        /// File the allowed model list is written to
        #[arg(long)]
        allowed: PathBuf,
        /// File the alias routes are written to
        #[arg(long)]
        aliases: PathBuf,
        /// File the backend aliases are written to
        #[arg(long)]
        backend: PathBuf,
        /// File the declared capability lifetime in whole seconds is written to
        #[arg(long)]
        lifetime: PathBuf,
    },
    /// Create the first inference-route registry at PATH from
    /// BRAMA_MODEL_ALIASES
    Seed {
        /// Registry file to create
        path: PathBuf,
    },
    /// The client identities the gateway preloads, read from the vault
    #[command(subcommand)]
    Identities(IdentityCommand),
    /// One non-empty field of the vault item that plays a role, printed bare
    Field {
        /// The `skarbiec-entitlements-router` executable that reads the vault
        #[arg(long)]
        router: PathBuf,
        /// Role whose item to read: the one live item tagged `stado:role:<ROLE>`
        role: String,
        /// Field of that item to print
        field: String,
    },
    /// Check that the pinned broker advertises every router command path the
    /// launcher and these launcher steps invoke
    Verbs {
        /// The pinned `skarbiec-entitlements-router` executable to ask
        #[arg(long)]
        router: PathBuf,
        /// The launcher script; it and the `.sh` stages in its `launcher/`
        /// directory are scanned for router command paths
        #[arg(long)]
        launcher: PathBuf,
    },
}

/// `brama launcher identities`: the client tables the gateway preloads.
#[derive(Subcommand)]
pub(crate) enum IdentityCommand {
    /// The preloaded model-router client table (JSON) for BRAMA_ALLOWED_MODELS
    Router {
        /// The `skarbiec-entitlements-router` executable that reads the vault
        #[arg(long)]
        router: PathBuf,
        /// JSON list of the backend client's exact aliases
        #[arg(long)]
        backend_models: String,
    },
    /// Every product's request-sign identity (JSON object)
    Sign {
        /// The `skarbiec-entitlements-router` executable that reads the vault
        #[arg(long)]
        router: PathBuf,
    },
}

fn printed(result: Result<String, String>) -> Result<(), String> {
    result.map(|text| print!("{text}"))
}

pub(crate) fn run(command: LauncherCommand) {
    let result = match command {
        LauncherCommand::Catalog {
            available,
            policy,
            output,
        } => catalog::build(&available, &policy, &output),
        LauncherCommand::Policy {
            config,
            allowed,
            aliases,
            backend,
            lifetime,
        } => policy::check(
            &config,
            &policy::Outputs {
                allowed: &allowed,
                aliases: &aliases,
                backend: &backend,
                lifetime: &lifetime,
            },
        ),
        LauncherCommand::Seed { path } => std::env::var("BRAMA_MODEL_ALIASES")
            .map_err(|error| {
                format!("BRAMA_MODEL_ALIASES is required and could not be read: {error}")
            })
            .and_then(|aliases| policy::seed_routes(&path, &aliases)),
        LauncherCommand::Identities(IdentityCommand::Router {
            router,
            backend_models,
        }) => std::env::var("BRAMA_ALLOWED_MODELS")
            .map_err(|error| {
                format!("BRAMA_ALLOWED_MODELS is required and could not be read: {error}")
            })
            .and_then(|allowed| {
                printed(identities::model_router(&router, &allowed, &backend_models))
            }),
        LauncherCommand::Identities(IdentityCommand::Sign { router }) => {
            printed(identities::request_sign(&router))
        }
        LauncherCommand::Field {
            router,
            role,
            field,
        } => printed(identities::vault::role_field(&router, &role, &field)),
        LauncherCommand::Verbs { router, launcher } => verbs::check(&router, &launcher),
    };
    if let Err(detail) = result {
        crate::cli::refused(detail);
    }
}
