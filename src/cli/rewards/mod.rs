//! `brama rewards`: this gateway's side of Wisent's contribution ledger from an
//! operator shell.
//!
//! `status` says which ledger this gateway declares and, for an alias, whether
//! it is gated and what the ledger answers about an agent's entitlement to
//! it. Brama links no one: a signed-in user links their account on the
//! Rewards page of compute.wisent.com, which reads it from that sign-in, and
//! a workload's credential, presented by every process that holds it, links
//! nothing.

use clap::{Args, Subcommand};

use brama::gateway::broker::rewards::status;

#[derive(Subcommand)]
pub(crate) enum RewardsCommand {
    /// Report the ledger this gateway declares and, for an alias, an agent's entitlement to it
    Status(StatusArgs),
}

#[derive(Args)]
pub(crate) struct StatusArgs {
    /// The alias to report: whether it is gated, and with --agent-id the entitlement
    #[arg(long)]
    alias: Option<String>,
    /// The agent id whose entitlement to the alias to ask the ledger about
    #[arg(long, requires = "alias")]
    agent_id: Option<String>,
    /// Print the report as JSON
    #[arg(long, default_value_t = false)]
    json: bool,
}

pub(crate) async fn run(command: RewardsCommand) {
    match command {
        RewardsCommand::Status(args) => {
            let report = match status(args.alias.as_deref(), args.agent_id.as_deref()).await {
                Ok(report) => report,
                Err(failure) => super::refused(failure),
            };
            super::print_answer(&report, args.json);
        }
    }
}
