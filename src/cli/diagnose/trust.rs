//! Whether one installed generation can redeem a capability: its bundled
//! router knows the verb the launcher calls, and the trust registry pins the
//! uid, gid, absolute path and SHA-256 of exactly this binary.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::cli::workload::pins::mismatches;

/// The router verb path the launcher needs from a bundled broker.
pub(super) const CAPABILITY_COMMAND: [&str; 2] = ["grant", "capability"];

pub(super) fn router_answers(root: &Path) -> bool {
    let router = root.join("bin/skarbiec-entitlements-router");
    if !router.is_file() {
        return false;
    }
    let Ok(output) = Command::new(&router)
        .args([CAPABILITY_COMMAND[0], "help"])
        .env("SKARBIEC_VAULT_FILE", root.join("no-such-vault.json"))
        .output()
    else {
        return false;
    };
    let document: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    output.status.success()
        && document
            .get("commands")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|command| {
                command
                    .split_whitespace()
                    .take(CAPABILITY_COMMAND.len())
                    .eq(CAPABILITY_COMMAND.iter().copied())
            })
}

pub(super) fn registry_verdict(root: &Path, config_dir: &Path) -> Vec<String> {
    let registry = config_dir.join("registry.json");
    if !registry.is_file() {
        return vec![format!("{}: absent", registry.display())];
    }
    match mismatches(&registry, &root.join("bin/brama")) {
        Ok(wrong) if wrong.is_empty() => {
            vec![format!(
                "{}: describes this installation",
                registry.display()
            )]
        }
        Ok(wrong) => wrong,
        Err(error) => vec![format!("{}: {error}", registry.display())],
    }
}
