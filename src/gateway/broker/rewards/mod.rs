//! Brama's side of Wisent's contribution ledger.
//!
//! The ledger lives in compute.wisent.com: it credits contributions and sells
//! entitlements for the credits. Brama reports the contribution it sees — a
//! donated subscription serving the shared pool — and asks the ledger, before
//! it routes a gated alias, whether the caller holds the entitlement to it.
//!
//! A gateway that declares no ledger (`BRAMA_CONTRIBUTION_LEDGER_URL`) reports
//! nothing and gates nothing: every gateway works without the ledger. One that
//! declares it reads its reporter bearer through the Skarbiec route for the
//! `contribution-ledger` resource. A gated alias is never served on a ledger
//! that did not answer; an ungated one never waits for it.

mod ledger;
mod pooled;
mod status;

pub use ledger::{check_entitlement, gated, ledger_url, Gate, Unanswered};
pub(crate) use pooled::note_served;
pub use status::{status, StatusFailure};

/// The name this gateway reports and asks under; the ledger's operator
/// declares it with the bearer's hash.
pub(crate) const PRODUCT: &str = "brama";

/// What a signed-in Wisent user's account agent begins with; the rest is the
/// account id in simple form (`subscriptions::account::account_agent_id`).
const ACCOUNT_AGENT: &str = "user-";

/// The identity the ledger knows the owner of `agent` by: the Wisent account
/// id of a signed-in user's account agent, which the account links on its
/// own Rewards page with no code; any other agent as it is, which no one
/// links, since a workload's credential is presented by every process that
/// holds it. An account agent whose id is no account id is refused naming it.
pub(crate) fn contributor_of(agent: &str) -> Result<String, String> {
    let Some(simple) = agent.strip_prefix(ACCOUNT_AGENT) else {
        return Ok(agent.to_string());
    };
    uuid::Uuid::try_parse(simple)
        .map(|account| account.to_string())
        .map_err(|error| format!("account agent {agent} names no Wisent account id: {error}"))
}
