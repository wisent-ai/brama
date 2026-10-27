//! Whether the adapter a Skarbiec call would go through is already held:
//! Stado's resolver publishes every relayed connection it is still waiting
//! on beside its state (`resolver-waiting.json`, the shape Stado's Machine
//! Interface page declares), and a call sent to a bind that holds one would
//! stand behind it without end. Brama asked Skarbiec on every completion
//! and, when the vault behind the adapter stopped answering, every
//! completion stood with it: the judge connections nobody could end. A call
//! to a held bind is refused at once with the held wait named, so the
//! caller is told what stands and where instead of joining it.

use std::path::PathBuf;

/// One wait the resolver publishes. Older records omit `bind` and `phase`;
/// unrelated newer fields are tolerated.
#[derive(serde::Deserialize)]
struct PendingChannel {
    service: String,
    consumer: String,
    #[serde(default)]
    bind: String,
    active_host: String,
    endpoint: String,
    since: String,
    phase: Option<String>,
}

/// Where the resolver publishes its waits: beside its state, whose place is
/// Stado's own contract (`readiness::standing::resolver_state_file`).
fn waiting_file() -> Result<PathBuf, String> {
    Ok(
        crate::core::server::readiness::standing::resolver_state_file()?
            .with_file_name("resolver-waiting.json"),
    )
}

/// Why a call to `url` would stand, when this machine's resolver already
/// holds a connection on that bind unanswered: every such wait named with
/// its service, consumer, phase, endpoint, host and start. `Ok(None)` when
/// the bind holds nothing, or the resolver has published no list; `Err`
/// when the list's place cannot be named, or it is there and cannot be
/// read, because then nobody can say whether the call would be answered.
pub(super) fn held_at(url: &str) -> Result<Option<String>, String> {
    let Some(authority) = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
    else {
        return Ok(None);
    };
    let path = waiting_file()?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "cannot inspect the resolver's waiting list {}: {error}",
                path.display()
            ))
        }
    };
    let waits: Vec<PendingChannel> = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "cannot decode the resolver's waiting list {}: {error}",
            path.display()
        )
    })?;
    let pending: Vec<String> = waits
        .iter()
        .filter(|wait| wait.bind == authority)
        .map(|wait| {
            format!(
                "service {} consumer {} phase {} to {} on {} since {}",
                wait.service,
                wait.consumer,
                match wait.phase.as_deref() {
                    Some(phase) => phase,
                    None => "open",
                },
                wait.endpoint,
                wait.active_host,
                wait.since,
            )
        })
        .collect();
    if pending.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "not sent to {url}: this machine's resolver ({}) holds {} unanswered connection(s) on \
         that adapter, and a call sent now would stand behind them: {}; inspect the named \
         service and `stado resolver status`",
        path.display(),
        pending.len(),
        pending.join("; ")
    )))
}
