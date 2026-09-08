//! How much of the pool one proven identity may be told about.
//!
//! The pool is one document, and which rows it carries follows from what the
//! caller proved rather than from which route was asked. An operator console,
//! an account holder and a signed agent put the same question; the answers
//! differ because the identities do, not because three implementations of the
//! answer exist beside each other.
//!
//! It is separate from the document itself because the narrowing is a rule
//! about callers, and every read in this module has to consult it: the listing,
//! the historical accounts only the ledger remembers, and the sentence the
//! document prints about what it answered.

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PoolScope {
    /// Every account this deployment holds, whichever agent owns it, together
    /// with the accounts only the usage ledger still remembers. Nothing short
    /// of this installation's own console proves this much.
    Deployment,
    /// Exactly the accounts one agent owns.
    Agent(String),
}

impl PoolScope {
    pub(super) fn agent(&self) -> Option<&str> {
        match self {
            Self::Deployment => None,
            Self::Agent(agent_id) => Some(agent_id),
        }
    }

    /// What the document says it answered. A caller reads the narrowing here
    /// instead of inferring it from how many rows came back.
    pub fn named(&self) -> &str {
        match self {
            Self::Deployment => "deployment",
            Self::Agent(agent_id) => agent_id,
        }
    }
}
