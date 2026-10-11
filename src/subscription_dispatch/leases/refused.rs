//! A lease holder's report that the provider refused the member it leased.
//!
//! A harness that keeps its own sign-in (OMP) talks to its provider directly,
//! so a rate limit it meets never passes through this gateway and the usage
//! ledger would keep leasing the refused member. The holder reports it: the
//! member is blocked until the instant the provider named, and the session's
//! leases on that provider are released, so the lease it takes next lands on
//! a usable member of the same provider (another account with capacity), or
//! the pool's shortage starts an acquisition.

use super::{register, Lease};

/// Why a report was not taken.
#[derive(Debug)]
pub enum ReportRefused {
    /// The report is malformed: a blank field, or an instant already past.
    Invalid(String),
    /// The session holds no live lease on the provider for this caller.
    NotHeld(String),
    /// The register could not be read or written.
    Unavailable(String),
}

/// What a taken report did: the leases whose members are blocked until
/// `until_ms`, every one of them released.
#[derive(Debug)]
pub struct Reported {
    pub blocked: Vec<Lease>,
    pub until_ms: i64,
}

/// Block every member `session_id` holds a live lease on for `provider`
/// until `until_ms`, with the provider's `reason`, and release those leases,
/// as `agent` (none for the console): an agent reports only its own leases.
pub fn report(
    session_id: &str,
    provider: &str,
    until_ms: i64,
    reason: &str,
    agent: Option<&str>,
    now_ms: i64,
) -> Result<Reported, ReportRefused> {
    for (name, value) in [
        ("session_id", session_id),
        ("provider", provider),
        ("reason", reason),
    ] {
        if value.trim().is_empty() {
            return Err(ReportRefused::Invalid(format!(
                "a refusal report names the {name}; it was empty"
            )));
        }
    }
    if until_ms <= now_ms {
        return Err(ReportRefused::Invalid(format!(
            "the provider's retry instant {until_ms} is not after now ({now_ms}); a refusal whose wait is over blocks nothing"
        )));
    }
    let held: Vec<Lease> = register::live()
        .map_err(ReportRefused::Unavailable)?
        .into_iter()
        .filter(|lease| lease.session_id == session_id && lease.provider == provider)
        .filter(|lease| lease.belongs_to(agent))
        .collect();
    if held.is_empty() {
        return Err(ReportRefused::NotHeld(format!(
            "session {session_id} holds no live {provider} lease this caller took, so no member is known to be refused"
        )));
    }
    for lease in &held {
        crate::subscription_dispatch::usage::record_reported_block(
            &lease.subscription_id,
            provider,
            reason,
            until_ms,
        );
        register::release(Some(&lease.id), None, agent).map_err(|refused| {
            ReportRefused::Unavailable(format!(
                "member {} is blocked, but lease {} was not released: {refused:?}",
                lease.subscription_id, lease.id
            ))
        })?;
    }
    Ok(Reported {
        blocked: held,
        until_ms,
    })
}
