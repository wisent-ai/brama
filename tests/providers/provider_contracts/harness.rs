//! What a provider contract story runs against: the real binary with the whole
//! operator environment handed to it, the seeded vault and usage ledger, and
//! the journal records the state directory holds afterwards.

use std::process::{Command, Output};

use serde_json::Value;

use super::AGENT;
use crate::support::{SkarbiecVault, TestDirectory};

/// One command against one real vault and one test-owned state area. The
/// vault's environment carries HOME, GNUPGHOME and the vault path, and Brama
/// hands its whole environment to the real router child.
pub(crate) fn run(
    directory: &TestDirectory,
    vault: &SkarbiecVault,
    environment: &[(&str, &str)],
    args: &[&str],
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    command
        .env_remove("WELES_API_TOKEN")
        .env_remove("WELES_WORKER_ENV_FILE")
        .env_remove("BRAMA_SUBSCRIPTION_CATALOG")
        .env_remove("BRAMA_STADO_BIN")
        .env_remove("BRAMA_WELES_URL")
        .env_remove("BRAMA_WELES_REAUTH_TOKEN");
    for (name, value) in vault.environment() {
        command.env(name, value);
    }
    command
        .env("XDG_STATE_HOME", directory.path().join("xdg-state"))
        .env("BRAMA_STATE_DIR", directory.path().join("state"))
        .env(
            "BRAMA_SUBSCRIPTION_USAGE_FILE",
            directory.path().join("usage.json"),
        )
        .env("ENTITLEMENTS_ROUTER_BIN", vault.router())
        .envs(environment.iter().copied())
        .args(args)
        .output()
        .expect("run the real brama binary")
}

/// One never-touched subscription per provider, id `probe-<provider>`, in both
/// places a subscription has to exist to be one: the vault that declares it
/// and the usage ledger that records what is known about it.
pub(crate) fn seed_ledger(directory: &TestDirectory, vault: &SkarbiecVault, providers: &[&str]) {
    let rows: Vec<String> = providers
        .iter()
        .map(|provider| format!(r#""probe-{provider}":{{"provider":"{provider}"}}"#))
        .collect();
    std::fs::write(
        directory.path().join("usage.json"),
        format!(r#"{{"subscriptions":{{{}}}}}"#, rows.join(",")),
    )
    .expect("seed usage ledger");
    for provider in providers {
        vault.seed_subscription(AGENT, provider, &format!("probe-{provider}"));
    }
}

pub(crate) fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub(crate) fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The journal records the state directory holds, newest last.
pub(crate) fn journal_records(directory: &TestDirectory) -> Vec<Value> {
    let path = directory.path().join("state").join("journal.jsonl");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("journal line is JSON"))
        .collect()
}
