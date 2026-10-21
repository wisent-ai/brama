//! The credential calls this gateway has sent and that stand unanswered.
//!
//! Every chat is admitted on Skarbiec's answer about its bearer. When
//! Skarbiec accepts the call and never answers, `/readyz` still returned the
//! last sweep's verdict, so a client that waits only while Brama declares it
//! serves waited without end. Each call to the credential source is recorded
//! here while it stands. A call that has stood the refresh interval this
//! host's Stado resolver declares (the measure the resolver reads its own
//! waits by) makes `/readyz` declare that the gateway does not serve, naming
//! the source and how long it has stood; the same reading ends every such
//! call with that sentence, so a request already accepted is answered with
//! the refusal instead of standing on. A reading happens when readiness is
//! asked, and [`watch`] takes one itself whenever a call begins or ends and
//! at the moment the oldest call reaches the interval ([`next_reading`]),
//! whether or not any client is asking: a request accepted while nobody
//! watches `/readyz` is refused on time all the same. A question waiting
//! for the answer to change is woken by the same events.

mod listing;
mod measure;

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::oneshot;

pub(in crate::core::server) use listing::{readings, CallReading};
use measure::declared_measure;
pub(in crate::core::server) use measure::{measure_document, resolver_state_file};

/// One call standing: where it went, which operation it is and which
/// process runs it when it is a vault child, since when (measured, and on
/// the wall clock for a reader), and the channel its waiter is told on when
/// a reading ends it.
struct Call {
    target: String,
    operation: String,
    pid: Option<u32>,
    since: Instant,
    began: SystemTime,
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
    /// Record `operation`, a call to `target`, as standing from now; `pid`
    /// is the process it runs in when the gateway shelled it.
    pub(crate) fn begin(target: &str, operation: &str, pid: Option<u32>) -> Self {
        let marker = Arc::new(());
        let (end, ended) = oneshot::channel();
        let call = Call {
            target: target.to_string(),
            operation: operation.to_string(),
            pid,
            since: Instant::now(),
            began: SystemTime::now(),
            end: Some(end),
        };
        standing().insert(Arc::as_ptr(&marker) as usize, call);
        super::changed();
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
        super::changed();
    }
}

/// When the oldest call still standing reaches the declared measure: the
/// one moment a reading changes the readiness answer with nothing else
/// changing. `None` when no call stands unended or nothing declares a
/// measure. A declaration that cannot be read is the readiness answer's own
/// refusal already, so it sets no moment either; the cause is said in the
/// log.
pub(in crate::core::server) fn next_reading() -> Option<Instant> {
    let oldest = standing()
        .values()
        .filter(|call| call.end.is_some())
        .map(|call| call.since)
        .min()?;
    match declared_measure() {
        Ok(Some(measure)) => Some(oldest + Duration::from_secs(measure.seconds)),
        Ok(None) => None,
        Err(cause) => {
            tracing::warn!("no moment to read standing credential calls by: {cause}");
            None
        }
    }
}

/// Why the gateway does not serve now, when a credential call has stood the
/// declared measure or longer. Each such call is ended with a sentence
/// naming that call — its operation, target, process and how long it stood
/// — so the request waiting on it is refused with its own cause; the
/// returned readiness sentence names how many stand and the oldest.
/// `Ok(None)` when nothing stands that long, or when nothing declares a
/// measure (`/readyz` says so in `standing_measure`); an error when a
/// declaration cannot be read, so the calls cannot be measured.
pub(in crate::core::server) fn refusal() -> Result<Option<String>, String> {
    let mut calls = standing();
    if calls.is_empty() {
        return Ok(None);
    }
    let Some(measure) = declared_measure().map_err(|cause| {
        format!(
            "{} credential call(s) stand and cannot be measured: {cause}",
            calls.len()
        )
    })?
    else {
        return Ok(None);
    };
    let limit = Duration::from_secs(measure.seconds);
    let mut stale: Vec<&mut Call> = calls
        .values_mut()
        .filter(|call| call.since.elapsed() >= limit)
        .collect();
    stale.sort_by_key(|call| call.since);
    let Some(oldest) = stale.first() else {
        return Ok(None);
    };
    let sentence = format!(
        "the credential source has not answered: {} call(s) have stood {}s ({}) or longer, the oldest to {} for {}s; Brama serves no chat until it answers",
        stale.len(),
        measure.seconds,
        measure.declared_by,
        oldest.target,
        oldest.since.elapsed().as_secs()
    );
    for call in stale {
        let process = match call.pid {
            Some(pid) => format!("pid {pid}"),
            None => "an HTTP request".to_string(),
        };
        let own = format!(
            "the credential source has not answered {} to {} ({process}) for {}s, past the {}s {} declares; Brama serves no chat until it answers",
            call.operation,
            call.target,
            call.since.elapsed().as_secs(),
            measure.seconds,
            measure.declared_by,
        );
        if let Some(end) = call.end.take() {
            if let Err(unsent) = end.send(own) {
                tracing::warn!(standing_call = %call.target, "a standing call's waiter went away before its refusal reached it: {unsent}");
            }
        }
    }
    Ok(Some(sentence))
}

/// The gateway's own readings of the standing calls, run for the life of
/// the process. It waits for a call to begin or end, or for the moment the
/// oldest unended call reaches the declared refresh interval, then reads:
/// every call past the interval is ended with the refusal `/readyz` gives,
/// so its request is answered with it, and the log says which calls were
/// refused and why. A reading that cannot measure the calls says that cause
/// in the log once, until the cause changes.
pub(in crate::core::server) async fn watch() {
    let mut said: Option<String> = None;
    loop {
        // Registered before the moment is read, so a call beginning between
        // the reading and the wait wakes this loop instead of passing it.
        let notified = super::CHANGED.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        match next_reading() {
            Some(due) => {
                tokio::select! {
                    () = &mut notified => {}
                    () = tokio::time::sleep_until(tokio::time::Instant::from_std(due)) => {}
                }
            }
            None => notified.await,
        }
        let reading = match refusal() {
            Ok(reading) => reading,
            Err(cause) => Some(cause),
        };
        if reading != said {
            if let Some(sentence) = &reading {
                tracing::warn!("standing credential calls refused by the gateway's own reading: {sentence}");
            }
            said = reading;
        }
    }
}
