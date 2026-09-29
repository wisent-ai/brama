//! Check every router command path the launcher uses against the pinned
//! broker's own inventory, before a release ships a launcher that calls a
//! verb the broker does not have.
//!
//! The paths come from two places: the literal `"$ENTITLEMENTS_ROUTER_BIN"
//! verb …` calls in the launcher and every stage file it sources, and the
//! calls this binary's own launcher steps make ([`OWN_CALLS`]). Only help
//! inventories are executed; a group name alone cannot prove that the
//! subcommand exists.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// How the launcher names the broker binary in a literal call.
const ROUTER_CALL: &str = "\"$ENTITLEMENTS_ROUTER_BIN\"";

/// A command word: a lowercase letter, then lowercase letters and dashes.
fn is_word(token: &str) -> bool {
    let mut chars = token.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c == '-')
}

/// Shell keywords after which the next word is a command.
const COMMAND_KEYWORDS: [&str; 9] = [
    "then", "do", "else", "exec", "!", "time", "if", "while", "until",
];

/// Whether the router literal that follows `before` is the command of a
/// simple command, rather than an argument (`--router "$…"`), a test operand
/// (`[ -x "$…" ]`) or an assignment value (`NAME="$…"`). Only a command runs
/// a broker verb; an argument is read by whatever program it is handed to.
fn in_command_position(before: &str) -> bool {
    let line = before.rsplit('\n').next().unwrap_or_default();
    let trimmed = line.trim_end();
    if trimmed.is_empty() {
        return true;
    }
    if trimmed.ends_with(['(', '|', ';', '&', '{', '`']) {
        return true;
    }
    if trimmed.len() == line.len() {
        return false;
    }
    let last = trimmed
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or_default();
    COMMAND_KEYWORDS.contains(&last) || is_assignment(last)
}

/// `NAME=value`: an environment prefix of the command that follows it.
fn is_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(name, _)| {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// The command words after each literal router call in command position:
/// every whitespace-separated word up to the first token that is not one.
fn shell_calls(text: &str) -> Vec<Vec<String>> {
    text.match_indices(ROUTER_CALL)
        .filter(|(at, _)| in_command_position(&text[..*at]))
        .map(|(at, _)| {
            text[at + ROUTER_CALL.len()..]
                .split_whitespace()
                .take_while(|token| is_word(token))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|words| !words.is_empty())
        .collect()
}

/// The router command paths `brama launcher` and `brama workload` invoke.
const OWN_CALLS: [&[&str]; 2] = [&["get"], &["grant", "issue"]];

fn inventory(router: &Path, args: &[&str], vault: &Path) -> Result<Value, String> {
    let output = Command::new(router)
        .args(args)
        .env("SKARBIEC_VAULT_FILE", vault)
        .output()
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

/// The launcher and every stage file beside it that `sh` sources.
fn family(launcher: &Path) -> Vec<PathBuf> {
    fn walk(directory: &Path, into: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                walk(&path, into);
            } else if path.extension().is_some_and(|ext| ext == "sh") {
                into.push(path);
            }
        }
    }
    let mut files = vec![launcher.to_path_buf()];
    walk(
        &launcher.parent().unwrap_or(Path::new(".")).join("launcher"),
        &mut files,
    );
    files
}

pub(super) fn check(router: &Path, launcher: &Path) -> Result<(), String> {
    let vault = router
        .parent()
        .unwrap_or(Path::new("."))
        .join("no-such-vault.json");
    let root = inventory(router, &["help"], &vault)?;
    let groups = strings(root.get("groups"));
    if root.get("groups").and_then(Value::as_array).is_none() {
        return Err(
            "the pinned broker does not declare CLI groups; use Skarbiec 0.3.2 or newer".into(),
        );
    }
    let first = |command: &str, words: usize| {
        command
            .split_whitespace()
            .take(words)
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut available: BTreeSet<String> = strings(root.get("commands"))
        .iter()
        .map(|command| first(command, 1))
        .collect();
    for group in &groups {
        let document = inventory(router, &[group, "help"], &vault)?;
        available.extend(
            strings(document.get("commands"))
                .iter()
                .map(|command| first(command, 2)),
        );
    }
    let mut calls: Vec<Vec<String>> = OWN_CALLS
        .iter()
        .map(|call| call.iter().map(|word| word.to_string()).collect())
        .collect();
    for path in family(launcher) {
        let text = std::fs::read(&path)
            .map_err(|error| format!("{}: {error}", path.display()))
            .and_then(|bytes| {
                String::from_utf8(bytes).map_err(|_| {
                    format!("{} is not a launcher: it is not UTF-8 text; the launcher and router arguments are in the wrong order", path.display())
                })
            })?;
        let text = text.replace("\\\n", " ");
        calls.extend(shell_calls(&text));
    }
    let required: BTreeSet<String> = calls
        .iter()
        .map(|words| {
            let take = if groups.contains(&words[0]) { 2 } else { 1 };
            words
                .iter()
                .take(take)
                .cloned()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    println!(
        "launcher requires: {}",
        required.iter().cloned().collect::<Vec<_>>().join(", ")
    );
    let missing: Vec<&String> = required.difference(&available).collect();
    if !missing.is_empty() {
        let names: Vec<&str> = missing.iter().map(|name| name.as_str()).collect();
        return Err(format!(
            "the pinned broker does not implement: {}",
            names.join(", ")
        ));
    }
    println!("the pinned broker advertises every literal command path the launcher invokes");
    Ok(())
}
