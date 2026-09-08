//! Running the local entitlements router, and reading what it refused with.
//!
//! Every credential this gateway issues, reads, writes or lists arrives
//! through one child process under one timeout. That is one subject on its
//! own: the binary's name, the bound on how long it may take, and the sentence
//! a non-zero exit turns into are identical for a capability issue, a grant
//! read, an item write and a vault listing, and a second copy of any of them
//! would drift from this one.

use std::time::Duration;

const ENTITLEMENTS_ROUTER_BIN_ENV: &str = "ENTITLEMENTS_ROUTER_BIN";
const DEFAULT_ENTITLEMENTS_ROUTER_BIN: &str = "entitlements-router";
pub(super) const ENTITLEMENTS_ROUTER_TIMEOUT: Duration = Duration::from_secs(15);

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
