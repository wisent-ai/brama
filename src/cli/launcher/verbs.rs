//! Check every router command path the launcher uses against the pinned
//! broker's own inventory, before a release ships a launcher that calls a
//! verb the broker does not have.
//!
//! The paths come from two places: the literal `"$ENTITLEMENTS_ROUTER_BIN"
//! verb …` calls in the launcher and every stage file it sources, read with
//! the shell grammar ([`shell`]), and the calls this binary's own launcher
//! steps make ([`own_calls`]). Only help inventories are executed; a group
//! name alone cannot prove that the subcommand exists.

mod lexer;
mod shell;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// How the launcher names the broker binary in a literal call.
const ROUTER_CALL: &str = "\"$ENTITLEMENTS_ROUTER_BIN\"";

/// The command line `brama launcher` sends the router for an item.
const GET_CALL: &str = "get";
/// The command line `brama workload` sends the router to issue a grant.
const GRANT_ISSUE_CALL: &str = "grant issue";

/// The router command paths `brama launcher` and `brama workload` invoke.
fn own_calls() -> Vec<Vec<String>> {
    let words =
        |call: &str| -> Vec<String> { call.split_whitespace().map(str::to_string).collect() };
    vec![words(GET_CALL), words(GRANT_ISSUE_CALL)]
}

fn inventory(router: &Path, args: &[&str], vault: &Path) -> Result<Value, String> {
    let output = stado_wait::output(
        Command::new(router)
            .args(args)
            .env("SKARBIEC_VAULT_FILE", vault),
    )
    .map_err(|error| format!("broker inventory {} failed: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "broker inventory {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let document: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("broker inventory {} is not JSON: {error}", args.join(" ")))?;
    if !document.get("commands").is_some_and(Value::is_array) {
        return Err(format!(
            "broker inventory {} has no commands",
            args.join(" ")
        ));
    }
    Ok(document)
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// A command path as the broker's inventory spells it: the verb, or, under a
/// group, the group and its verb. A grouped call that names no verb is the
/// group alone, which no inventory lists, so the check names it missing.
fn command_path<S: AsRef<str>>(words: &[S], grouped: bool) -> String {
    match words {
        [group, verb, ..] if grouped => format!("{} {}", group.as_ref(), verb.as_ref()),
        [verb, ..] => verb.as_ref().to_string(),
        [] => String::new(),
    }
}

/// The directory `path` lies in (empty, the working directory, for a bare
/// file name), refused by name when it has none, since the files beside it
/// are what the check reads.
fn directory_of<'a>(path: &'a Path, what: &str) -> Result<&'a Path, String> {
    path.parent().ok_or_else(|| {
        format!(
            "the {what} {} names no file inside a directory; pass the path to the file",
            path.display()
        )
    })
}

/// The launcher and every stage file beside it that `sh` sources. An absent
/// stage directory holds none; one that cannot be listed, or an entry that
/// cannot be read, refuses, so the launcher is never checked against fewer
/// calls than it makes.
fn family(launcher: &Path) -> Result<Vec<PathBuf>, String> {
    fn walk(directory: &Path, into: &mut Vec<PathBuf>) -> Result<(), String> {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("cannot list {}: {error}", directory.display())),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!("cannot read an entry of {}: {error}", directory.display())
            })?;
            paths.push(entry.path());
        }
        paths.sort();
        for path in paths {
            if path.is_dir() {
                walk(&path, into)?;
            } else if path.extension().is_some_and(|ext| ext == "sh") {
                into.push(path);
            }
        }
        Ok(())
    }
    let mut files = vec![launcher.to_path_buf()];
    walk(&directory_of(launcher, "launcher")?.join("launcher"), &mut files)?;
    Ok(files)
}

pub(super) fn check(router: &Path, launcher: &Path) -> Result<(), String> {
    let vault = directory_of(router, "router")?.join("no-such-vault.json");
    let root = inventory(router, &["help"], &vault)?;
    if root.get("groups").and_then(Value::as_array).is_none() {
        return Err(
            "the pinned broker does not declare CLI groups; use Skarbiec 0.3.2 or newer".into(),
        );
    }
    let groups = strings(root.get("groups"));
    let words = |command: &str| -> Vec<String> {
        command.split_whitespace().map(str::to_string).collect()
    };
    let mut available: BTreeSet<String> = strings(root.get("commands"))
        .iter()
        .map(|command| command_path(&words(command), false))
        .collect();
    for group in &groups {
        let document = inventory(router, &[group, "help"], &vault)?;
        available.extend(
            strings(document.get("commands"))
                .iter()
                .map(|command| command_path(&words(command), true)),
        );
    }
    let mut calls: Vec<Vec<String>> = own_calls();
    for path in family(launcher)? {
        let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let text = String::from_utf8(bytes).map_err(|error| {
            format!(
                "{} is not a launcher: it is not UTF-8 text ({error}); the launcher and router arguments are in the wrong order",
                path.display()
            )
        })?;
        calls.extend(
            shell::router_calls(&text, ROUTER_CALL)
                .map_err(|error| format!("{}: {error}", path.display()))?,
        );
    }
    let required: BTreeSet<String> = calls
        .iter()
        .map(|call| {
            let grouped = call.first().is_some_and(|word| groups.contains(word));
            command_path(call, grouped)
        })
        .collect();
    println!(
        "launcher requires: {}",
        required.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    let missing: Vec<&str> = required
        .difference(&available)
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "the pinned broker does not implement: {}",
            missing.join(", ")
        ));
    }
    println!("the pinned broker advertises every literal command path the launcher invokes");
    Ok(())
}
