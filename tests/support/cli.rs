//! The built binary as an operator runs it, pointed at a scratch directory
//! and an entitlements router that does not exist. Every environment variable
//! here is one the product reads, so a test never inherits the operator's own
//! state.

use std::process::Command;

use crate::support::TestDirectory;

pub fn command(directory: &TestDirectory) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    command
        .env("HOME", directory.path().join("home"))
        .env("XDG_STATE_HOME", directory.path().join("xdg-state"))
        .env("BRAMA_STATE_DIR", directory.path().join("state"))
        .env(
            "BRAMA_SUBSCRIPTION_USAGE_FILE",
            directory.path().join("usage.json"),
        )
        .env(
            "BRAMA_DONATED_SUBSCRIPTIONS_FILE",
            directory.path().join("subscriptions.json"),
        )
        .env("BRAMA_PERF_PATH", directory.path().join("perf.json"))
        .env(
            "ENTITLEMENTS_ROUTER_BIN",
            directory.path().join("absent-router"),
        );
    command
}
