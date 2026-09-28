//! The `brama` command line, one module per command family.

pub(crate) mod adoption;
pub(crate) mod aliases;
pub(crate) mod catalogue;
pub(crate) mod diagnose;
pub(crate) mod decisions;
pub(crate) mod diagnostics;
pub(crate) mod launcher;
pub(crate) mod media;
pub(crate) mod onboarding;
pub(crate) mod probe;
pub(crate) mod serving;
pub(crate) mod stub;
pub(crate) mod subscriptions;
pub(crate) mod version_gate;
pub(crate) mod workload;

use serde_json::Value;

/// One report as the desktop console consumes it.
fn print_json(report: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".into())
    );
}
