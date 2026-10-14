//! Running the local entitlements router, and reading what it refused with.
//!
//! Every credential this gateway issues, reads, writes or lists arrives
//! through one child process under one timeout. That is one subject on its
//! own: the binary's name, the bound on how long it may take, and the sentence
//! a non-zero exit turns into are identical for a capability issue, a grant
//! read, an item write and a vault listing, and a second copy of any of them
//! would drift from this one.

use std::sync::{Arc, LazyLock, Mutex};

use futures_util::future::{BoxFuture, FutureExt, Shared};

mod executable;

pub(crate) use executable::entitlements_router_bin;
use executable::{ENTITLEMENTS_ROUTER_BIN_ENV, SKARBIEC_BIN_ENV};

/// The router's bare `list`: the JSON row of every vault item.
///
/// Every reader of the vault's inventory — per-agent discovery, the console's
/// pool, the readiness sweep's unroutable-account census, the sign-in loop,
/// and each credential write's tag lookup — shells the same `list`, and the
/// router decrypts and parses the whole vault to answer it. With hundreds of
/// items in the vault and the readiness sweep and the sign-in loop asking
/// concurrently every half minute, the router alone can hold most of a
/// gigabyte and half a core beside a saturated gateway, until the host's own
/// object store closes connections for want of memory. Concurrent callers
/// share the one listing in flight instead of each starting a router.
///
/// Shared while in flight, never after: the vault changes under this
/// gateway by hands it does not see — `skarbiec delete`, a vault sync, an
/// operator's `set` — and a listing served after it returned would answer
/// with an account the vault no longer holds. The `usage` story deletes a
/// real item mid-run and reads it as forgotten on the very next call.
type ListingResult = Result<Arc<Vec<u8>>, String>;

static IN_FLIGHT: LazyLock<Mutex<Option<Shared<BoxFuture<'static, ListingResult>>>>> =
    LazyLock::new(|| Mutex::new(None));

/// The slot holds one shared listing that is only ever replaced whole, so a
/// panic while it was held leaves nothing half-written: the lock is
/// recovered, which keeps listings served and a finished one cleared.
fn in_flight_listing(
) -> std::sync::MutexGuard<'static, Option<Shared<BoxFuture<'static, ListingResult>>>> {
    crate::core::sync::held(&IN_FLIGHT, "vault listing in flight")
}

/// The bare `list` of `binary`: the listing already in flight when one is,
/// otherwise a new one every concurrent caller joins. A refused or failed
/// listing is returned to every joined caller and kept by none.
pub(in crate::gateway::broker) async fn raw_listing(
    binary: &str,
    operation: &str,
) -> ListingResult {
    let listing = {
        let mut in_flight = in_flight_listing();
        match in_flight.as_ref() {
            Some(shared) => shared.clone(),
            None => {
                let binary = binary.to_owned();
                let operation = operation.to_owned();
                let shared = async move {
                    let output = child_output(&binary, &operation, |command| {
                        command.arg("list");
                    })
                    .await?;
                    if !output.status.success() {
                        return Err(router_refusal(&operation, &output));
                    }
                    Ok(Arc::new(output.stdout))
                }
                .boxed()
                .shared();
                *in_flight = Some(shared.clone());
                shared
            }
        }
    };
    let result = listing.clone().await;
    {
        let mut in_flight = in_flight_listing();
        // Only the listing that just finished is cleared; a newer one a later
        // caller started stays for the callers joining it.
        if in_flight
            .as_ref()
            .is_some_and(|current| current.ptr_eq(&listing))
        {
            *in_flight = None;
        }
    }
    result
}

/// Run one entitlements-router command and read its output.
///
/// The child's own exit is the answer. A vault read on a cold keychain and a
/// vault read that will never return look the same to a clock, and killing
/// the first one turns a credential that exists into a credential this
/// gateway reports as missing. Memory does tell them apart: see
/// [`wait_within_footprint`].
pub(in crate::gateway::broker) async fn child_output(
    binary: &str,
    operation: &str,
    configure: impl FnOnce(&mut tokio::process::Command),
) -> Result<std::process::Output, String> {
    let mut command = tokio::process::Command::new(binary);
    command
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    configure(&mut command);
    match command.spawn() {
        Ok(child) => wait_within_footprint(child, operation).await,
        // The program is part of the failure. `No such file or directory (os
        // error 2)` on its own sent three readers of `subscription sync` to
        // the vault, the grant and the pool before anyone asked which file
        // was missing, and the answer was the bare word this gateway spawns.
        Err(error) => Err(format!(
            "{operation}: {error} (running {binary}; declare another with \
             {ENTITLEMENTS_ROUTER_BIN_ENV} or {SKARBIEC_BIN_ENV})"
        )),
    }
}

/// The resident memory, in bytes, past which a vault child is ended, when the
/// operator declares one. A router child that had run for over a day under
/// this gateway once held 3 GiB while the control host refused all work for
/// memory pressure; the ceiling that fits a host is that host's to declare.
const RESIDENT_CEILING_ENV: &str = "BRAMA_VAULT_CHILD_RESIDENT_CEILING_BYTES";

/// How often, in whole seconds, a running vault child's footprint is read
/// against [`RESIDENT_CEILING_ENV`]. Required once a ceiling is declared.
const FOOTPRINT_CHECK_ENV: &str = "BRAMA_VAULT_CHILD_FOOTPRINT_CHECK_SECONDS";

/// The declared ceiling and the cadence it is read at, or `None` when no
/// ceiling is declared; a ceiling without a cadence is refused by name.
fn declared_ceiling() -> Result<Option<(u64, std::time::Duration)>, String> {
    let Ok(value) = std::env::var(RESIDENT_CEILING_ENV) else {
        return Ok(None);
    };
    let ceiling = value.trim().parse::<u64>().map_err(|_| {
        format!("{RESIDENT_CEILING_ENV} must be a whole number of bytes; it is {value:?}")
    })?;
    let cadence = crate::types::declared_age(FOOTPRINT_CHECK_ENV)?.ok_or_else(|| {
        format!("{RESIDENT_CEILING_ENV} is declared, so {FOOTPRINT_CHECK_ENV} must say how often it is read")
    })?;
    if cadence.is_zero() {
        return Err(format!("{FOOTPRINT_CHECK_ENV} must be at least one second"));
    }
    Ok(Some((ceiling, cadence)))
}

/// Wait for `child` to exit and collect its output. The log names the
/// operation and pid when the child starts and ends, so a command that never
/// ends is attributable from the log without any assumed duration. When the
/// operator declares a resident ceiling, the child is ended once it passes
/// it, and the refusal names the operation, the pid and the measured
/// footprint. The child also stands in the readiness register while it
/// runs: a vault that accepts the command and never answers left every
/// chat request on it unanswered until the client's transport gave up with
/// an empty reply, with `/readyz` still answering ready. A readiness reading
/// that finds the child standing longer than the refresh interval this
/// host's resolver declares ends it with the reading's own sentence, which
/// the request is then refused with, and `/readyz` says the same.
pub(in crate::gateway::broker) async fn wait_within_footprint(
    child: tokio::process::Child,
    operation: &str,
) -> Result<std::process::Output, String> {
    let pid = child.id();
    let ceiling = declared_ceiling().map_err(|refusal| format!("{operation}: {refusal}"))?;
    tracing::info!(operation, pid = ?pid, "vault child started");
    let pid_text = match pid {
        Some(pid) => pid.to_string(),
        None => "already reaped".to_string(),
    };
    let mut standing = crate::core::server::Standing::begin(&format!(
        "vault child `{operation}` (pid {pid_text})"
    ));
    let output = child.wait_with_output();
    tokio::pin!(output);
    let Some((ceiling, cadence)) = ceiling else {
        tokio::select! {
            finished = &mut output => {
                tracing::info!(operation, pid = ?pid, "vault child ended");
                return finished.map_err(|error| format!("{operation}: wait for the vault child: {error}"));
            }
            sentence = standing.ended() => {
                tracing::warn!(operation, pid = ?pid, "vault child abandoned by the readiness reading");
                // Dropping the pinned wait kills the child (kill_on_drop).
                return Err(format!("{operation}: vault child pid {pid_text} abandoned: {sentence}"));
            }
        }
    };
    let mut system = sysinfo::System::new();
    let mut checks = tokio::time::interval(cadence);
    loop {
        tokio::select! {
            finished = &mut output => {
                tracing::info!(operation, pid = ?pid, "vault child ended");
                return finished.map_err(|error| format!("{operation}: wait for the vault child: {error}"));
            }
            sentence = standing.ended() => {
                tracing::warn!(operation, pid = ?pid, "vault child abandoned by the readiness reading");
                return Err(format!("{operation}: vault child pid {pid_text} abandoned: {sentence}"));
            }
            _ = checks.tick() => {
                let Some(pid) = pid.map(sysinfo::Pid::from_u32) else { continue };
                if !system.refresh_process(pid) {
                    continue;
                }
                let Some(resident) = system.process(pid).map(sysinfo::Process::memory) else { continue };
                if resident > ceiling {
                    // Dropping the pinned wait kills the child (kill_on_drop).
                    return Err(format!(
                        "{operation}: vault child pid {pid} held {resident} bytes resident, over the \
                         {ceiling}-byte ceiling {RESIDENT_CEILING_ENV} declares; ended it"
                    ));
                }
            }
        }
    }
}

/// Hand `document` to a started router child on its stdin, then close it.
/// A write that fails stops the child, because one left running may store
/// the part it read; one that cannot be stopped is named in the refusal with
/// what it may have stored.
pub(in crate::gateway::broker) async fn feed_stdin(
    child: &mut tokio::process::Child,
    document: &[u8],
    operation: &str,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| format!("{operation}: the child's stdin is unavailable"))?;
    if let Err(error) = input.write_all(document).await {
        let stopped = match child.kill().await {
            Ok(()) => String::new(),
            Err(kill) => {
                format!("; the child could not be stopped and may store a partial document: {kill}")
            }
        };
        return Err(format!(
            "{operation}: writing the document to the child failed: {error}{stopped}"
        ));
    }
    Ok(())
}

pub(in crate::gateway::broker) async fn router_output(
    operation: &str,
    configure: impl FnOnce(&mut tokio::process::Command),
) -> Result<std::process::Output, String> {
    child_output(&entitlements_router_bin()?, operation, configure).await
}

pub(in crate::gateway::broker) fn router_refusal(
    operation: &str,
    output: &std::process::Output,
) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        format!(
            "{operation} failed with status {}; stderr was empty",
            output.status
        )
    } else {
        format!("{operation} failed with status {}: {detail}", output.status)
    }
}
