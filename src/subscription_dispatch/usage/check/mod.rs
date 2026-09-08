//! The verdict of one proactive check, and which mechanism's verdict a reader
//! is given.
//!
//! Two mechanisms answer two different questions about a subscription and each
//! keeps its own newest verdict. The provider's own usage report says whether
//! the account's ration can be read at all; it costs no quota, so it runs on a
//! timer, and it lives in `usage_report`. One minimal completion says whether
//! the provider will actually serve this credential; it costs quota, so it runs
//! only when an operator asks, and it lives in `completion`.
//!
//! They share this one verdict shape and nothing else. Keeping the two newest
//! verdicts apart is the whole point: ordinary traffic or a later completion
//! must never erase the reason the usage report itself could not be read, and a
//! blank plan whose report succeeded is a different fact from a blank plan
//! nothing ever managed to ask about.

mod completion;
mod usage_report;

use serde::{Deserialize, Serialize};

use super::SubscriptionUsage;

pub use completion::record_probe;
pub use usage_report::{
    record_plan_usage, record_plan_usage_failure, record_plan_usage_unpublished,
};

/// Which kind of check produced a verdict.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckSource {
    /// The provider's own usage report. Costs no quota, so this is what runs on
    /// a timer.
    UsageReport,
    /// One minimal completion, which costs quota and therefore only ever runs
    /// when an operator asks for it.
    Completion,
}

/// The outcome of one proactive check.
///
/// The same shape is stored independently for completion probes and provider
/// usage reports. `source` identifies the mechanism; the enclosing
/// `SubscriptionUsage` field decides which mechanism's newest verdict it is.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Probe {
    pub attempted_at_ms: i64,
    pub ok: bool,
    /// The provider's own sentence when it refused, trimmed like every other
    /// stored reason here. On success it is set only when the success itself
    /// needs explaining -- Brama has no supported free usage-report endpoint
    /// for this credential -- and absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Which check this verdict came from. Absent in ledgers written before the
    /// free usage report existed, where every verdict was a completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<CheckSource>,
}

/// The newest provider-only usage attempt, separate from completion probes.
///
/// Reading the older shared field keeps ledgers written before the dedicated
/// one readable until their next in-process load migrates the verdict.
pub fn plan_usage_check(recorded: Option<&SubscriptionUsage>) -> Option<&Probe> {
    let recorded = recorded?;
    recorded.usage_check.as_ref().or_else(|| {
        recorded
            .probe
            .as_ref()
            .filter(|probe| probe.source == Some(CheckSource::UsageReport))
    })
}
