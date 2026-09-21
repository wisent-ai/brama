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

const ENTITLEMENTS_ROUTER_BIN_ENV: &str = "ENTITLEMENTS_ROUTER_BIN";
/// The vault binary's second declaration, the one Skarbiec's own fixtures and
/// Stado's release journeys set.
const SKARBIEC_BIN_ENV: &str = "SKARBIEC_BIN";
const DEFAULT_ENTITLEMENTS_ROUTER_BIN: &str = "entitlements-router";
/// The installed vault CLI, in the two names it is reached by on this fleet.
const VAULT_PROGRAM_NAMES: [&str; 2] = [DEFAULT_ENTITLEMENTS_ROUTER_BIN, "skarbiec"];
/// Where the fleet installs it when nothing is on `PATH`, relative to `$HOME`.
const VAULT_HOME_RELATIVE: &str = ".stado/bin/skarbiec";


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

/// The vault binary this gateway shells for every credential operation.
///
/// Four places, in this order: the router's own declaration, the vault's
/// declaration, an executable of either name on `PATH`, and the path the
/// fleet installs it at. The bare word `entitlements-router` used to be the
/// whole answer, and on a machine where the vault is installed as
/// `~/.stado/bin/skarbiec` and nothing exports either variable, every
/// credential write failed with `No such file or directory (os error 2)` and
/// named no program: on 2026-09-20 that is what `brama subscription sync`
/// answered for all three grants this machine holds, while Oko's own
/// verification could not be judged for want of a working subscription.
pub(crate) fn entitlements_router_bin() -> String {
    for declaration in [ENTITLEMENTS_ROUTER_BIN_ENV, SKARBIEC_BIN_ENV] {
        if let Some(value) = std::env::var(declaration)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return value;
        }
    }
    if let Some(found) = VAULT_PROGRAM_NAMES.into_iter().find_map(on_path) {
        return found;
    }
    if let Some(home) = std::env::var_os("HOME") {
        let installed = std::path::Path::new(&home).join(VAULT_HOME_RELATIVE);
        if is_executable_file(&installed) {
            return installed.display().to_string();
        }
    }
    DEFAULT_ENTITLEMENTS_ROUTER_BIN.to_owned()
}

/// Is `path` a file this process could execute?
fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|info| info.is_file() && info.permissions().mode() & 0o111 != 0)
}

/// The first executable named `program` on `PATH`.
fn on_path(program: &str) -> Option<String> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(program))
        .find(|candidate| is_executable_file(candidate))
        .map(|candidate| candidate.display().to_string())
}

/// Run one entitlements-router command and read its output.
///
/// The child's own exit is the answer. A vault read on a cold keychain and a
/// vault read that will never return look the same to a clock, and killing
/// the first one turns a credential that exists into a credential this
/// gateway reports as missing.
pub(in crate::gateway::broker) async fn child_output(
    binary: &str,
    operation: &str,
    configure: impl FnOnce(&mut tokio::process::Command),
) -> Result<std::process::Output, String> {
    let mut command = tokio::process::Command::new(binary);
    command.kill_on_drop(true);
    configure(&mut command);
    match command.output().await {
        Ok(output) => Ok(output),
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

pub(in crate::gateway::broker) async fn router_output(
    operation: &str,
    configure: impl FnOnce(&mut tokio::process::Command),
) -> Result<std::process::Output, String> {
    child_output(&entitlements_router_bin(), operation, configure).await
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
