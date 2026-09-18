//! Running the local entitlements router, and reading what it refused with.
//!
//! Every credential this gateway issues, reads, writes or lists arrives
//! through one child process under one timeout. That is one subject on its
//! own: the binary's name, the bound on how long it may take, and the sentence
//! a non-zero exit turns into are identical for a capability issue, a grant
//! read, an item write and a vault listing, and a second copy of any of them
//! would drift from this one.

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

const ENTITLEMENTS_ROUTER_BIN_ENV: &str = "ENTITLEMENTS_ROUTER_BIN";
const DEFAULT_ENTITLEMENTS_ROUTER_BIN: &str = "entitlements-router";
pub(super) const ENTITLEMENTS_ROUTER_TIMEOUT: Duration = Duration::from_secs(15);

/// How long one raw vault listing answers every caller before the router is
/// asked again. Shorter than the per-agent subscription cache, because the
/// writers below read the listing to learn an item's tags before they write
/// and must see a member somebody banked a moment ago.
const RAW_LISTING_TTL: Duration = Duration::from_secs(10);

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
/// releases wrote to it. One listing now answers every caller inside its
/// window, and concurrent callers wait for the one in flight instead of
/// starting their own.
struct RawListing {
    fetched_at: Instant,
    stdout: Arc<Vec<u8>>,
}

static RAW_LISTING: LazyLock<tokio::sync::Mutex<Option<RawListing>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));

/// The bare `list` of `binary`, from the shared window when one is fresh.
/// A refused or failed listing is returned to its caller and never stored.
pub(in crate::gateway::broker) async fn raw_listing(
    binary: &str,
    operation: &str,
) -> Result<Arc<Vec<u8>>, String> {
    // The lock is held across the shell on purpose: it is what makes a second
    // caller wait for the listing in flight rather than start another.
    let mut cached = RAW_LISTING.lock().await;
    if let Some(listing) = cached.as_ref() {
        if listing.fetched_at.elapsed() < RAW_LISTING_TTL {
            return Ok(Arc::clone(&listing.stdout));
        }
    }
    let output = bounded_output(binary, operation, |command| {
        command.arg("list");
    })
    .await?;
    if !output.status.success() {
        return Err(router_refusal(operation, &output));
    }
    let stdout = Arc::new(output.stdout);
    *cached = Some(RawListing {
        fetched_at: Instant::now(),
        stdout: Arc::clone(&stdout),
    });
    Ok(stdout)
}

/// Forget the shared listing: a write just changed the vault, and the next
/// reader must see it.
pub(in crate::gateway::broker) async fn forget_raw_listing() {
    *RAW_LISTING.lock().await = None;
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
