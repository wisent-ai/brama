//! What can be presented to a provider on this agent's behalf, and what the
//! ledger already knows about it.
//!
//! `readiness` answers the question a request asks, without serving one;
//! `auth_failure` reads a provider's refusal for the difference between a
//! stale token and a dead grant; `eligibility` narrows a deployment's
//! subscription listing to the rows one call may be billed to; and
//! `usage_probe` spends one call on exactly one named account.

pub(super) mod auth_failure;
pub(super) mod eligibility;
pub(super) mod readiness;
pub(super) mod usage_probe;
