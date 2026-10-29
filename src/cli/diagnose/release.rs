//! Why this host's Brama stopped serving, as the release agent recorded it.
//!
//! A gateway with no process leaves nothing to ask: `/readyz` does not
//! answer, the unit's log ends mid-boot, and the one account of what killed
//! it is the release state Stado's agent keeps beside the release roles:
//! which release is active, when the active one lost readiness, and every
//! candidate digest it quarantined with the cause it named and the line it
//! read that cause from. That record is printed here, newest quarantine
//! first, with the commands that repair each kind of stop, so a dead gateway
//! is diagnosed from the file that says why rather than from its symptom.

use serde_json::Value;

use super::Layout;

/// The product name the release agent keys this host's state by.
const PRODUCT: &str = "brama";

/// A field of the release record as text, or the statement that the agent
/// did not record it.
fn recorded<'a>(record: &'a Value, field: &str) -> &'a str {
    match record.get(field).and_then(Value::as_str) {
        Some(text) if !text.is_empty() => text,
        _ => "not recorded",
    }
}

pub(super) fn print_release_outcome(layout: &Layout) {
    say!("\n=== why the release stopped serving");
    let state = &layout.release_state;
    if state.is_null() {
        say!("  no release state is recorded on this host, so no release agent has run Brama here");
        return;
    }
    say!("  phase: {}", recorded(state, "phase"));
    match state.get("active").filter(|active| !active.is_null()) {
        Some(active) => say!(
            "  active: {} (pid {}, since {})",
            recorded(active, "version"),
            active
                .get("pid")
                .map_or_else(|| "not recorded".to_string(), Value::to_string),
            recorded(active, "started_at")
        ),
        None => say!("  active: none, so no Brama release serves this host"),
    }
    if let Some(lost) = state.get("readiness_lost_at").and_then(Value::as_str) {
        say!("  the active release has not answered its readiness read since {lost}");
    }
    if let Some(detail) = state
        .get("detail")
        .and_then(Value::as_str)
        .filter(|detail| !detail.is_empty())
    {
        say!("  agent detail: {detail}");
    }
    let quarantined = match state.get("quarantined") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(records)) => records.iter().collect::<Vec<_>>(),
        Some(other) => {
            say!("  the quarantine map is not an object: {other}");
            return;
        }
    };
    if quarantined.is_empty() {
        say!("  quarantined candidates: none");
        return;
    }
    let mut quarantined = quarantined;
    // RFC 3339 instants in one offset order as text; the newest stop is the
    // one that explains the current state, so it is printed first.
    quarantined.sort_by(|(_, left), (_, right)| {
        recorded(right, "quarantined_at").cmp(recorded(left, "quarantined_at"))
    });
    say!("  quarantined candidates, newest first: {}", quarantined.len());
    for (digest, record) in &quarantined {
        say!(
            "  {digest}\n    at:       {}\n    cause:    {}\n    evidence: {}\n    reason:   {}",
            recorded(record, "quarantined_at"),
            recorded(record, "cause"),
            recorded(record, "evidence"),
            recorded(record, "reason")
        );
    }
    say!(
        "  repair: `stado release doctor {PRODUCT} --json` names each cause's remedy and whether \
         the release agent retires that record by itself; a cause that holds the candidate is \
         cleared after its repair with `stado release quarantine clear --digest <digest> --reason <text>`"
    );
}
