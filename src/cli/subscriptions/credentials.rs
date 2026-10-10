//! `brama subscription`: acting on one provider's subscription credentials --
//! renewing them now, signing the account in through Weles or by hand,
//! enrolling the authenticator that makes every later sign-in unattended, or
//! giving a pool member back.

mod run;

use clap::builder::NonEmptyStringValueParser;
use clap::{ArgGroup, Subcommand, ValueEnum};

pub(crate) use run::run;

/// Who logs the account in on a sign-in: Weles drives the provider's login
/// row, or the operator logs in in their own browser and pastes the code the
/// page shows. One operation with two methods, so one verb.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum SignInBy {
    Weles,
    Hand,
}

/// A coding-agent harness a bought account can be handed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum Harness {
    Omp,
}

#[derive(Subcommand)]
pub(crate) enum SubscriptionCommand {
    /// Report the subscription pool this gateway routes over: every member,
    /// its state, its usage window and its failure
    List(super::SubscriptionsArgs),
    /// Refresh this provider's subscription credentials now, here or on the
    /// gateway that holds them
    #[command(group(ArgGroup::new("destination").args(["gateway", "gateway_consumer"])))]
    Refresh {
        /// The provider whose grants should be refreshed (`codex`, `claude-code`, `kimi`)
        provider: String,
        /// Why this refresh is being run; recorded in the journal beside the verdict
        #[arg(long, value_parser = NonEmptyStringValueParser::new())]
        reason: String,
        /// The gateway to refresh; the console's bearer is read from stdin
        #[arg(long)]
        gateway: Option<String>,
        /// Resolve the gateway through Stado's service directory as this consumer
        #[arg(long)]
        gateway_consumer: Option<String>,
        /// Read the console's bearer from the vault item playing this role (its `token` field) instead of from stdin
        #[arg(long, value_name = "ROLE", requires = "destination")]
        bearer_role: Option<String>,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Sign one provider account in and prove it: `--by weles` drives the
    /// exact Weles sign-in row and proves it with a refresh; `--by hand`
    /// prints the provider's authorize page, reads the code it shows on
    /// stdin, stores the grant and proves it with one completion, here or on
    /// the gateway that serves the pool (a provider without a manual flow is
    /// refused by name)
    #[command(
        name = "sign-in",
        group(ArgGroup::new("destination").args(["gateway", "gateway_consumer"]).requires("bearer_role"))
    )]
    SignIn {
        /// The provider whose account should be signed in (`codex`, `claude-code`, `kimi`)
        provider: String,
        /// Who logs in: `weles` (its sign-in row) or `hand` (you, in your own browser); stated every time, never assumed
        #[arg(long, value_enum)]
        by: SignInBy,
        /// `--by weles`: the exact Weles sign-in row to drive; without it the single row Weles holds for the provider is used, and two or more are never guessed between
        #[arg(long)]
        login_item: Option<String>,
        /// Exact Brama subscription whose grant must be replaced and proved; required `--by hand`
        #[arg(long)]
        subscription_id: Option<String>,
        /// Why this sign-in is being run; recorded in the journal beside the verdict
        #[arg(long)]
        reason: String,
        /// `--by hand`: the gateway that serves the pool; it draws the page, takes the paste and stores the grant in its own vault
        #[arg(long)]
        gateway: Option<String>,
        /// `--by hand`: resolve the gateway through Stado's service directory as this consumer
        #[arg(long)]
        gateway_consumer: Option<String>,
        /// `--by hand` with a gateway: read the console's bearer from the vault item playing this role (its `token` field), since stdin carries the pasted code
        #[arg(long, value_name = "ROLE", requires = "destination")]
        bearer_role: Option<String>,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// The authenticator that answers each account's second factor: list
    /// what every account needs and holds, or enrol one for a subscription's
    /// login
    Authenticator {
        #[command(subcommand)]
        command: AuthenticatorCommand,
    },
    /// Record which account each pool member of one provider belongs to, read from the member's own grant, here or on the gateway that holds the vault
    #[command(group(ArgGroup::new("destination").args(["gateway", "gateway_consumer"])))]
    Attribute {
        /// The provider whose members should be attributed (`codex`, `claude-code`, `kimi`)
        provider: String,
        /// The gateway whose vault records the accounts; the console's bearer is read from stdin
        #[arg(long)]
        gateway: Option<String>,
        /// Resolve the gateway through Stado's service directory as this consumer
        #[arg(long)]
        gateway_consumer: Option<String>,
        /// Read the console's bearer from the vault item playing this role (its `token` field) instead of from stdin
        #[arg(long, value_name = "ROLE", requires = "destination")]
        bearer_role: Option<String>,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Put one retired pool member back in the rotation, because this deployment uses that account after all
    #[command(
        name = "reinstate",
        group(ArgGroup::new("destination").args(["gateway", "gateway_consumer"]))
    )]
    Reinstate {
        /// The retired pool member to use again
        #[arg(long)]
        subscription_id: String,
        /// Why it is used again; recorded beside the gateway's own ledger entry
        #[arg(long, value_parser = NonEmptyStringValueParser::new())]
        reason: String,
        /// The gateway holding it; the console's bearer is read from stdin
        #[arg(long)]
        gateway: Option<String>,
        /// Resolve the gateway through Stado's service directory as this consumer
        #[arg(long)]
        gateway_consumer: Option<String>,
        /// Read the console's bearer from the vault item playing this role (its `token` field) instead of from stdin
        #[arg(long, value_name = "ROLE", requires = "destination")]
        bearer_role: Option<String>,
        /// Print `{subscription_id, detail}` as JSON instead of a line
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Give one pool member back: the gateway retires it and forgets its credential, and any machine that signed that account in keeps its own session
    #[command(
        name = "disown",
        group(ArgGroup::new("destination").args(["gateway", "gateway_consumer"]))
    )]
    Disown {
        /// The pool member to give back
        #[arg(long)]
        subscription_id: String,
        /// Why it is given back; recorded beside the gateway's own ledger entry
        #[arg(long, value_parser = NonEmptyStringValueParser::new())]
        reason: String,
        /// The gateway holding it; the console's bearer is read from stdin
        #[arg(long)]
        gateway: Option<String>,
        /// Resolve the gateway through Stado's service directory as this consumer
        #[arg(long)]
        gateway_consumer: Option<String>,
        /// Read the console's bearer from the vault item playing this role (its `token` field) instead of from stdin
        #[arg(long, value_name = "ROLE", requires = "destination")]
        bearer_role: Option<String>,
        /// Print `{subscription_id, detail}` as JSON instead of a line
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Buy one new account of this provider when every account in the pool
    /// has spent its plan and the pool holds fewer accounts than the
    /// operator allows: Weles creates and pays for it on the plan the pool's
    /// accounts hold, Brama proves its grant; here or on the gateway that
    /// holds the vault
    #[command(
        name = "acquire",
        group(ArgGroup::new("destination").args(["gateway", "gateway_consumer"]))
    )]
    Acquire {
        /// The provider to buy an account of: one src/subscription_dispatch/acquire/providers.json declares
        provider: String,
        /// Why an account is being bought; recorded in the journal beside the verdict
        #[arg(long, value_parser = NonEmptyStringValueParser::new())]
        reason: String,
        /// The gateway holding the pool; the console's bearer is read from stdin
        #[arg(long)]
        gateway: Option<String>,
        /// Resolve the gateway through Stado's service directory as this consumer
        #[arg(long)]
        gateway_consumer: Option<String>,
        /// Read the console's bearer from the vault item playing this role (its `token` field) instead of from stdin
        #[arg(long, value_name = "ROLE", requires = "destination")]
        bearer_role: Option<String>,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Sign every bought account into a coding-agent harness on this
    /// machine: each harness grant Weles stored for an acquired account that
    /// this harness does not hold yet is imported through the harness's own
    /// credential import, then marked as handed over in the vault
    #[command(name = "hand-over")]
    HandOver {
        /// The provider whose bought accounts are handed over: one providers.json declares for the harness
        provider: String,
        /// The harness to sign in (`omp`)
        #[arg(long, value_enum)]
        harness: Harness,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Which live session runs on which subscription: take a lease for a
    /// session before its harness starts, release it when the terminal ends,
    /// list every live lease; always on the gateway that holds the pool
    #[command(subcommand)]
    Lease(LeaseCommand),
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
        /// Print the leases as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum AuthenticatorCommand {
    /// List which accounts need a second factor to be signed in, and which of them hold the secret that answers one
    List {
        /// Narrow the list to one provider; without it every provider is listed
        #[arg(long)]
        provider: Option<String>,
        /// Print the list as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Enrol an authenticator for the login behind one subscription, so every later sign-in answers the provider's second factor by itself
    Enrol {
        /// The provider whose account needs a seed: claude-code, codex or kimi
        provider: String,
        /// Exact Brama subscription whose login gains the authenticator
        #[arg(long)]
        subscription_id: String,
        /// Why this enrolment is being run; recorded beside the verdict
        #[arg(long)]
        reason: String,
        /// Exact Skarbiec login item, when the subscription names more than one
        #[arg(long)]
        login_item: Option<String>,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}
