//! Whether one installed generation can redeem a capability: its bundled
//! router knows the verb the launcher calls, and the trust registry pins the
//! uid, gid, absolute path and SHA-256 of exactly this binary.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

use crate::cli::workload::pins::mismatches;

/// The router command group and verb the launcher calls on a bundled broker.
const CAPABILITY_GROUP: &str = "grant";
const CAPABILITY_VERB: &str = "capability";
/// The router verb path the launcher needs from a bundled broker.
pub(super) const CAPABILITY_COMMAND: &[&str] = &[CAPABILITY_GROUP, CAPABILITY_VERB];

/// Whether the generation's bundled router knows the capability verb, said
/// as the diagnosis prints it: `answers`, or what stopped it — a missing
/// router, one that cannot be started, one that failed, or one whose help is
/// not the JSON command list — so the line names the fault, not `false`.
pub(super) fn router_answers(root: &Path) -> String {
    let router = root.join("bin/skarbiec-entitlements-router");
    if !router.is_file() {
        return format!("no: {} is not a file", router.display());
    }
    let mut help = Command::new(&router);
    help.args([CAPABILITY_GROUP, "help"])
        .env("SKARBIEC_VAULT_FILE", root.join("no-such-vault.json"));
    let output = match stado_wait::output(&mut help) {
        Ok(output) => output,
        Err(error) => return format!("no: {} could not be run: {error}", router.display()),
    };
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        return format!(
            "no: `{CAPABILITY_GROUP} help` {}: {}",
            output.status,
            said.trim()
        );
    }
    let document: Value = match serde_json::from_slice(&output.stdout) {
        Ok(document) => document,
        Err(error) => {
            return format!(
                "no: `{CAPABILITY_GROUP} help` did not answer the JSON command list: {error}"
            )
        }
    };
    let knows = document
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
        });
    if knows {
        "answers".to_string()
    } else {
        format!(
            "no: its command list names no `{}`",
            CAPABILITY_COMMAND.join(" ")
        )
    }
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
