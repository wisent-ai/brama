//! The credential calls this gateway has sent and that stand unanswered.
//!
//! Every chat is admitted on Skarbiec's answer about its bearer. When
//! Skarbiec accepts the call and never answers, `/readyz` still returned the
//! last sweep's verdict, so a client that waits only while Brama declares it
//! serves waited without end. Each call to the credential source is recorded
//! here while it stands. A call standing longer than the refresh interval
//! this host's Stado resolver declares (the measure the resolver reads its
//! own waits by) makes `/readyz` declare that the gateway does not serve,
//! naming the source and how long it has stood; the same reading ends every
//! such call with that sentence, so a request already accepted is answered
//! with the refusal instead of standing on. Nothing here keeps a clock of
//! its own: the reading happens when readiness is asked.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use tokio::sync::oneshot;

/// One call standing: where it went, since when, and the channel its waiter
/// is told on when a reading ends it.
struct Call {
    target: String,
    since: Instant,
    end: Option<oneshot::Sender<String>>,
}

/// The standing calls, keyed by the address of each call's own marker.
static STANDING: LazyLock<Mutex<BTreeMap<usize, Call>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

fn standing() -> std::sync::MutexGuard<'static, BTreeMap<usize, Call>> {
    crate::core::sync::held(&STANDING, "standing calls")
}

/// A call recorded as standing until it is dropped: a credential call to
/// Skarbiec over HTTP, or a vault child this gateway shells and waits on,
/// which stands the same way when the vault on this host answers nothing.
pub(crate) struct Standing {
    marker: Arc<()>,
    ended: oneshot::Receiver<String>,
}

impl Standing {
    /// Record a call to `target` as standing from now.
    pub(crate) fn begin(target: &str) -> Self {
        let marker = Arc::new(());
        let (end, ended) = oneshot::channel();
        let call = Call {
            target: target.to_string(),
            since: Instant::now(),
            end: Some(end),
        };
        standing().insert(Arc::as_ptr(&marker) as usize, call);
        Self { marker, ended }
    }

    /// Resolves once a readiness reading ends this call, with its sentence.
    /// The sender lives in the standing record until this call drops, so a
    /// closed channel means the record was lost; that is answered with a
    /// sentence naming it rather than a wait that never resolves.
    pub(crate) async fn ended(&mut self) -> String {
        match (&mut self.ended).await {
            Ok(sentence) => sentence,
            Err(closed) => format!(
                "the standing-call record was lost before any readiness reading ended it: {closed}"
            ),
        }
    }
}

impl Drop for Standing {
    fn drop(&mut self) {
        standing().remove(&(Arc::as_ptr(&self.marker) as usize));
    }
}

/// Where Stado's resolver keeps its state on this machine: Stado's own
/// contract, overridable the way Stado's readers override it. A process
/// with neither the override nor a home cannot say where that is.
pub(in crate::core::server) fn resolver_state_file() -> Result<PathBuf, String> {
    if let Some(path) =
        std::env::var_os("STADO_RESOLVER_STATE_FILE").filter(|path| !path.is_empty())
    {
        return Ok(PathBuf::from(path));
    }
    match std::env::var_os("HOME").filter(|home| !home.is_empty()) {
        Some(home) => Ok(PathBuf::from(home).join(".stado").join("resolver-state.json")),
        None => Err("neither STADO_RESOLVER_STATE_FILE nor HOME names where this host's \
                     Stado resolver keeps its state"
            .to_string()),
    }
}

/// One JSON document of the resolver's: `None` when the file is absent, an
/// error naming the file when it cannot be read or is not JSON.
fn resolver_document(path: &PathBuf) -> Result<Option<serde_json::Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| format!("{} is not JSON: {error}", path.display()))
}

/// The refresh interval, in seconds, this host's resolver declares: the
/// target its state names, looked up in Stado's last-known-good registry
/// copy beside it. `None` when either file is absent or the target declares
/// no interval, which leaves no measure to read a standing call by; a file
/// that cannot be read, or a state and registry that do not name the target,
/// is an error naming which.
fn declared_refresh() -> Result<Option<u64>, String> {
    let state_file = resolver_state_file()?;
    let Some(state) = resolver_document(&state_file)? else {
        return Ok(None);
    };
    let target = state["target"].as_str().ok_or_else(|| {
        format!("{} names no target text", state_file.display())
    })?;
    let registry_file = state_file
        .with_file_name("cache")
        .join("registry-last-good.json");
    let Some(registry) = resolver_document(&registry_file)? else {
        return Ok(None);
    };
    let targets = registry["targets"].as_array().ok_or_else(|| {
        format!("{} holds no targets list", registry_file.display())
    })?;
    let entry = targets
        .iter()
        .find(|entry| entry["name"].as_str() == Some(target))
        .ok_or_else(|| {
            format!(
                "{} lists no target {target}, the one {} names",
                registry_file.display(),
                state_file.display()
            )
        })?;
    Ok(entry["service_resolver"]["refresh_seconds"].as_u64())
}

/// Why the gateway does not serve now, when a credential call has stood
/// longer than the declared refresh interval; every such call is ended with
/// the same sentence. `Ok(None)` when nothing stands that long, or when no
/// interval is declared to read a standing call by; an error when the
/// resolver's declaration cannot be read, so the calls cannot be measured.
pub(in crate::core::server) fn refusal() -> Result<Option<String>, String> {
    let mut calls = standing();
    if calls.is_empty() {
        return Ok(None);
    }
    let Some(refresh) = declared_refresh().map_err(|cause| {
        format!(
            "{} credential call(s) stand and cannot be measured: the resolver's refresh interval is unreadable: {cause}",
            calls.len()
        )
    })?
    else {
        return Ok(None);
    };
    let stale: Vec<&mut Call> = calls
        .values_mut()
        .filter(|call| call.since.elapsed().as_secs() > refresh)
        .collect();
    let Some(oldest) = stale.iter().max_by_key(|call| call.since.elapsed()) else {
        return Ok(None);
    };
    let sentence = format!(
        "the credential source has not answered: {} call(s) stand longer than the {refresh}s refresh interval this host's resolver declares, the oldest to {} for {}s; Brama serves no chat until it answers",
        stale.len(),
        oldest.target,
        oldest.since.elapsed().as_secs()
    );
    for call in stale {
        if let Some(end) = call.end.take() {
            if let Err(unsent) = end.send(sentence.clone()) {
                tracing::warn!(standing_call = %call.target, "a standing call's waiter went away before its refusal reached it: {unsent}");
            }
        }
    }
    Ok(Some(sentence))
}
