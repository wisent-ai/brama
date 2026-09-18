//! Running the local entitlements router, and reading what it refused with.
//!
//! Every credential this gateway issues, reads, writes or lists arrives
//! through one child process under one timeout. That is one subject on its
//! own: the binary's name, the bound on how long it may take, and the sentence
//! a non-zero exit turns into are identical for a capability issue, a grant
//! read, an item write and a vault listing, and a second copy of any of them
//! would drift from this one.

use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use futures_util::future::{BoxFuture, FutureExt, Shared};

const ENTITLEMENTS_ROUTER_BIN_ENV: &str = "ENTITLEMENTS_ROUTER_BIN";
const DEFAULT_ENTITLEMENTS_ROUTER_BIN: &str = "entitlements-router";
pub(super) const ENTITLEMENTS_ROUTER_TIMEOUT: Duration = Duration::from_secs(15);

/// The router's bare `list`: the JSON row of every vault item.
///
/// Every reader of the vault's inventory — per-agent discovery, the console's
/// pool, the readiness sweep's unroutable-account census, the sign-in loop,
/// and each credential write's tag lookup — shells the same `list`, and the
/// router decrypts and parses the whole vault to answer it. On the 16 GiB
/// control host on 2026-09-18 the vault held 654 items, the readiness sweep
/// and the sign-in loop asked concurrently every 30 seconds, the router sat
/// at 954 MiB and 58% CPU beside a gateway at 100%, and the host's own
/// object store closed connections for want of memory while the fleet's
/// releases wrote to it. Concurrent callers now share the one listing in
/// flight instead of each starting a router.
///
/// Shared while in flight, never after: the vault changes under this
/// gateway by hands it does not see — `skarbiec delete`, a vault sync, an
/// operator's `set` — and a listing served after it returned would answer
/// with an account the vault no longer holds. The `usage` story deletes a
/// real item mid-run and reads it as forgotten on the very next call.
type ListingResult = Result<Arc<Vec<u8>>, String>;

static IN_FLIGHT: LazyLock<Mutex<Option<Shared<BoxFuture<'static, ListingResult>>>>> =
    LazyLock::new(|| Mutex::new(None));

/// The bare `list` of `binary`: the listing already in flight when one is,
/// otherwise a new one every concurrent caller joins. A refused or failed
/// listing is returned to every joined caller and kept by none.
pub(in crate::gateway::broker) async fn raw_listing(
    binary: &str,
    operation: &str,
) -> ListingResult {
    let listing = {
        let mut in_flight = IN_FLIGHT
            .lock()
            .map_err(|_| "raw vault listing lock poisoned".to_owned())?;
        match in_flight.as_ref() {
            Some(shared) => shared.clone(),
            None => {
                let binary = binary.to_owned();
                let operation = operation.to_owned();
                let shared = async move {
                    let output = bounded_output(&binary, &operation, |command| {
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
    if let Ok(mut in_flight) = IN_FLIGHT.lock() {
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

pub(in crate::gateway::broker) fn entitlements_router_bin() -> String {
    std::env::var(ENTITLEMENTS_ROUTER_BIN_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ENTITLEMENTS_ROUTER_BIN.to_owned())
}

pub(in crate::gateway::broker) async fn bounded_output(
    binary: &str,
    operation: &str,
    configure: impl FnOnce(&mut tokio::process::Command),
) -> Result<std::process::Output, String> {
    let mut command = tokio::process::Command::new(binary);
    command.kill_on_drop(true);
    configure(&mut command);
    match tokio::time::timeout(ENTITLEMENTS_ROUTER_TIMEOUT, command.output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(format!("{operation}: {error}")),
        Err(_) => Err(format!(
            "{operation} timed out after {} seconds; the child was killed",
            ENTITLEMENTS_ROUTER_TIMEOUT.as_secs()
        )),
    }
}

pub(in crate::gateway::broker) async fn router_output(
    operation: &str,
    configure: impl FnOnce(&mut tokio::process::Command),
) -> Result<std::process::Output, String> {
    bounded_output(&entitlements_router_bin(), operation, configure).await
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
