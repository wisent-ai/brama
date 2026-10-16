//! Carrying out `brama subscription lease`: taking, releasing and listing the
//! leases that bind one session to one subscription of the pool.

use clap::builder::NonEmptyStringValueParser;
use clap::{ArgGroup, Subcommand};

use super::super::{leases, remote};

/// Carry out one lease command against the gateway it names.
pub(super) async fn run(command: LeaseCommand) {
    match command {
        LeaseCommand::Take {
            provider,
            session_id,
            holder,
            gateway,
            gateway_consumer,
            bearer_role,
            agent,
            signing_role,
            json,
        } => {
            leases::take(
                remote::Destination {
                    gateway,
                    gateway_consumer,
                    bearer_role,
                },
                &provider,
                &session_id,
                &holder,
                agent.as_deref().zip(signing_role.as_deref()),
                json,
            )
            .await
        }
        LeaseCommand::Release {
            lease_id,
            session_id,
            gateway,
            gateway_consumer,
            bearer_role,
            agent,
            signing_role,
            json,
        } => {
            leases::release(
                remote::Destination {
                    gateway,
                    gateway_consumer,
                    bearer_role,
                },
                lease_id.as_deref(),
                session_id.as_deref(),
                agent.as_deref().zip(signing_role.as_deref()),
                json,
            )
            .await
        }
        LeaseCommand::List {
            gateway,
            gateway_consumer,
            bearer_role,
            agent,
            signing_role,
            json,
        } => {
            leases::list(
                remote::Destination {
                    gateway,
                    gateway_consumer,
                    bearer_role,
                },
                agent.as_deref().zip(signing_role.as_deref()),
                json,
            )
            .await
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum LeaseCommand {
    /// Take a lease for one session on the usable subscription of a provider carrying the fewest sessions; a full pool is refused and an acquisition started for it
    #[command(group(ArgGroup::new("destination").args(["gateway", "gateway_consumer"]).required(true)))]
    Take {
        /// The provider the session will run on (`claude-code`)
        provider: String,
        /// The session that will run on the lease, as its runtime names it
        #[arg(long, value_parser = NonEmptyStringValueParser::new())]
        session_id: String,
        /// The program taking the lease for the session (`oko`)
        #[arg(long, value_parser = NonEmptyStringValueParser::new())]
        holder: String,
        /// The gateway holding the pool; the console's bearer is read from stdin
        #[arg(long)]
        gateway: Option<String>,
        /// Resolve the gateway through Stado's service directory as this consumer
        #[arg(long)]
        gateway_consumer: Option<String>,
        /// Read the console's bearer from the vault item playing this role (its `token` field) instead of from stdin
        #[arg(long, value_name = "ROLE")]
        bearer_role: Option<String>,
        /// Sign the request as this agent; requires --signing-role
        #[arg(long, value_parser = NonEmptyStringValueParser::new(), requires = "signing_role")]
        agent: Option<String>,
        /// Vault role and field of this agent's request-signing secret (ROLE#FIELD)
        #[arg(long, value_name = "ROLE#FIELD", value_parser = NonEmptyStringValueParser::new(), requires = "agent")]
        signing_role: Option<String>,
        /// Print the lease as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Release one lease by id, or every live lease of one session
    #[command(
        group(ArgGroup::new("destination").args(["gateway", "gateway_consumer"]).required(true)),
        group(ArgGroup::new("which").args(["lease_id", "session_id"]).required(true))
    )]
    Release {
        /// The lease to release, as `take` printed it
        #[arg(long)]
        lease_id: Option<String>,
        /// The session whose every live lease ends
        #[arg(long)]
        session_id: Option<String>,
        /// The gateway holding the pool; the console's bearer is read from stdin
        #[arg(long)]
        gateway: Option<String>,
        /// Resolve the gateway through Stado's service directory as this consumer
        #[arg(long)]
        gateway_consumer: Option<String>,
        /// Read the console's bearer from the vault item playing this role (its `token` field) instead of from stdin
        #[arg(long, value_name = "ROLE")]
        bearer_role: Option<String>,
        /// Sign the request as this agent; requires --signing-role
        #[arg(long, value_parser = NonEmptyStringValueParser::new(), requires = "signing_role")]
        agent: Option<String>,
        /// Vault role and field of this agent's request-signing secret (ROLE#FIELD)
        #[arg(long, value_name = "ROLE#FIELD", value_parser = NonEmptyStringValueParser::new(), requires = "agent")]
        signing_role: Option<String>,
        /// Print the released leases as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Every live lease, and how many sessions each subscription carries
    #[command(group(ArgGroup::new("destination").args(["gateway", "gateway_consumer"]).required(true)))]
    List {
        /// The gateway holding the pool; the console's bearer is read from stdin
        #[arg(long)]
        gateway: Option<String>,
        /// Resolve the gateway through Stado's service directory as this consumer
        #[arg(long)]
        gateway_consumer: Option<String>,
        /// Read the console's bearer from the vault item playing this role (its `token` field) instead of from stdin
        #[arg(long, value_name = "ROLE")]
        bearer_role: Option<String>,
        /// Sign the request as this agent; requires --signing-role
        #[arg(long, value_parser = NonEmptyStringValueParser::new(), requires = "signing_role")]
        agent: Option<String>,
        /// Vault role and field of this agent's request-signing secret (ROLE#FIELD)
        #[arg(long, value_name = "ROLE#FIELD", value_parser = NonEmptyStringValueParser::new(), requires = "agent")]
        signing_role: Option<String>,
        /// Print the leases as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}
