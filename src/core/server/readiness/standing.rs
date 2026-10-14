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

    /// Resolves once a readiness reading ends this call, with its sentence;
    /// a call no reading ends never resolves here.
    pub(crate) async fn ended(&mut self) -> String {
        match (&mut self.ended).await {
            Ok(sentence) => sentence,
            Err(_) => std::future::pending().await,
        }
    }
}

impl Drop for Standing {
    fn drop(&mut self) {
        standing().remove(&(Arc::as_ptr(&self.marker) as usize));
    }
}

/// Where Stado's resolver keeps its state on this machine: Stado's own
/// contract, overridable the way Stado's readers override it.
pub(in crate::core::server) fn resolver_state_file() -> Option<PathBuf> {
    match std::env::var_os("STADO_RESOLVER_STATE_FILE").filter(|path| !path.is_empty()) {
        Some(path) => Some(PathBuf::from(path)),
        None => Some(
            PathBuf::from(std::env::var_os("HOME")?)
                .join(".stado")
                .join("resolver-state.json"),
        ),
    }
}

/// The refresh interval, in seconds, this host's resolver declares: the
/// target its state names, looked up in Stado's last-known-good registry
/// copy beside it. `None` when either is absent or declares none, which
/// leaves no measure to read a standing call by.
fn declared_refresh() -> Option<u64> {
    let state_file = resolver_state_file()?;
    let read = |path: &PathBuf| -> Option<serde_json::Value> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    };
    let state = read(&state_file)?;
    let target = state["target"].as_str()?.to_string();
    let registry = read(
        &state_file
            .with_file_name("cache")
            .join("registry-last-good.json"),
    )?;
    registry["targets"]
        .as_array()?
        .iter()
        .find(|entry| entry["name"].as_str() == Some(target.as_str()))?["service_resolver"]
        ["refresh_seconds"]
        .as_u64()
}

/// Why the gateway does not serve now, when a credential call has stood
/// longer than the declared refresh interval; every such call is ended with
/// the same sentence. `None` when nothing stands that long, or when no
/// interval is declared to read a standing call by.
pub(in crate::core::server) fn refusal() -> Option<String> {
    let mut calls = standing();
    if calls.is_empty() {
        return None;
    }
    let refresh = declared_refresh()?;
    let stale: Vec<&mut Call> = calls
        .values_mut()
        .filter(|call| call.since.elapsed().as_secs() > refresh)
        .collect();
    let oldest = stale.iter().max_by_key(|call| call.since.elapsed())?;
    let sentence = format!(
        "the credential source has not answered: {} call(s) stand longer than the {refresh}s refresh interval this host's resolver declares, the oldest to {} for {}s; Brama serves no chat until it answers",
        stale.len(),
        oldest.target,
        oldest.since.elapsed().as_secs()
    );
    for call in stale {
        if let Some(end) = call.end.take() {
            // A waiter that already went away leaves nobody to tell.
            let _ = end.send(sentence.clone());
        }
    }
    Some(sentence)
}
