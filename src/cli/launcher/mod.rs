//! `brama launcher …`: what `start-with-skarbiec` needs computed before the
//! gateway starts, done by the binary it is about to start rather than by an
//! interpreter the host may not have.

mod catalog;

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
}

pub(crate) fn run(command: LauncherCommand) {
    let result = match command {
        LauncherCommand::Catalog { available, policy, output } => catalog::build(&available, &policy, &output),
    };
    if let Err(detail) = result {
        eprintln!("{detail}");
        std::process::exit(1);
    }
}
