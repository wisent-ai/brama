//! Compare the version `Cargo.toml` declares with the one the rule requires,
//! allowing for release coordinates a failed build consumed.
//!
//! The Stado release plane binds one coordinate to one source revision before
//! anything is published into it, so a build that fails at a coordinate
//! leaves it consumed and the next attempt needs the following patch. A
//! declared version may therefore run ahead of the required one in the same
//! series, provided every coordinate it skips was declared by an earlier
//! commit on this branch — the manifest history is the record of what was
//! submitted. A jump nothing accounts for is refused, naming the first
//! coordinate never declared.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use super::rule::{decide, Change, Decision};
use super::surface::{git_show, manifest_version};

pub(super) const BASELINE: &str = "released-surface.json";
const BREAKAGE: &str = "declared-breakage.json";
const MANIFEST: &str = "Cargo.toml";
/// The self-check's required version is one patch past the released one; its
/// unaccounted jump skips the two patches after that.
const REQUIRED_STEP: u64 = 1;
const UNACCOUNTED_STEP: u64 = 3;

pub(super) fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

fn triple(version: &str) -> Result<[u64; 3], String> {
    let invalid = || format!("{version} is not MAJOR.MINOR.PATCH");
    let parts: Vec<u64> = version
        .split('.')
        .map(|part| part.parse::<u64>())
        .collect::<Result<_, _>>()
        .map_err(|_| invalid())?;
    <[u64; 3]>::try_from(parts).map_err(|_| invalid())
}

/// The baseline's version and surface.
pub(super) fn released(root: &Path) -> Result<(String, Vec<String>), String> {
    let baseline = read_json(&root.join(BASELINE))?;
    let version = baseline["version"]
        .as_str()
        .filter(|version| !version.is_empty())
        .ok_or(format!("{BASELINE} names no released version"))?;
    let surface: Vec<String> = baseline["surface"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if surface.is_empty() {
        return Err(format!("{BASELINE} carries no surface"));
    }
    Ok((version.to_string(), surface))
}

/// Whether the owner declared, for this very version, a break the surface
/// cannot show. It expires when the manifest moves past that version.
fn declared_breakage(root: &Path, declared: &str) -> Result<bool, String> {
    let path = root.join(BREAKAGE);
    if !path.is_file() {
        return Ok(false);
    }
    let declaration = read_json(&path)?;
    if declaration["reason"]
        .as_str()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        return Err(format!(
            "{BREAKAGE} names no reason, so nothing here says what broke"
        ));
    }
    Ok(declaration["version"].as_str() == Some(declared))
}

/// Every version an earlier commit on this branch committed to Cargo.toml.
fn versions_declared_in_history(root: &Path) -> Result<BTreeSet<String>, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["log", "--format=%H", "--", MANIFEST])
        .output()
        .map_err(|error| format!("git log: {error}"))?;
    let commits: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if !output.status.success() || commits.len() <= 1 {
        return Err("the manifest history is one commit deep; fetch the full history before comparing versions".into());
    }
    Ok(commits[1..]
        .iter()
        .filter_map(|commit| git_show(root, commit, MANIFEST).ok())
        .filter_map(|text| manifest_version(&text).ok())
        .collect())
}

fn compare(
    declared: &str,
    released: &str,
    change: Change,
    required: &str,
    history: &BTreeSet<String>,
) -> Result<String, String> {
    let class = change.name();
    let satisfied = format!(
        "Committed manifest version {declared} satisfies the {class} public-surface change from {released}"
    );
    if declared == released {
        if change != Change::Internal {
            return Err(format!("Cargo.toml commits {declared}, but the public surface is {class} and requires {required}."));
        }
        return Ok(format!("{satisfied}."));
    }
    if declared == required {
        return Ok(format!("{satisfied}."));
    }
    let (want, have) = (triple(required)?, triple(declared)?);
    if have[..2] != want[..2] || have[2] <= want[2] {
        return Err(format!("Cargo.toml commits {declared}, but the {class} change from {released} requires {required}."));
    }
    let skipped: Vec<String> = (want[2]..have[2])
        .map(|patch| format!("{}.{}.{patch}", want[0], want[1]))
        .collect();
    if let Some(coordinate) = skipped
        .iter()
        .find(|coordinate| !history.contains(*coordinate))
    {
        return Err(format!("Cargo.toml commits {declared}, but the {class} change from {released} requires {required}, and {coordinate} was never declared by an earlier commit on this branch, so nothing consumed it."));
    }
    Ok(format!(
        "{satisfied}: {required} is required and {} were consumed by earlier commits.",
        skipped.join(", ")
    ))
}

fn decision(
    released: &str,
    published: &[String],
    candidate: &[String],
    breaking: bool,
) -> Result<Decision, String> {
    decide(released, published, candidate, breaking).map_err(|refusal| refusal.to_string())
}

/// Prove the rule and the comparison can both refuse before trusting either.
fn self_check(released: &str, published: &[String]) -> Result<(), String> {
    let unchanged = decision(released, published, published, false)?.change;
    let broken = decision(released, published, &published[1..], false)?.change;
    if unchanged != Change::Internal || broken != Change::Breaking {
        return Err(format!(
            "self-check returned identical={} removed={}",
            unchanged.name(),
            broken.name()
        ));
    }
    let [major, minor, patch] = triple(released)?;
    let required = format!("{major}.{minor}.{}", patch + REQUIRED_STEP);
    let far = format!("{major}.{minor}.{}", patch + UNACCOUNTED_STEP);
    match compare(&far, released, Change::Internal, &required, &BTreeSet::new()) {
        Err(_) => Ok(()),
        Ok(_) => Err(format!("the comparison accepted {far} against a required {required} with nothing consuming the gap.")),
    }
}

/// The whole gate: prints the verdict JSON and the comparison, or refuses.
pub(super) fn run(root: &Path, candidate: &[String], check_self: bool) -> Result<(), String> {
    let manifest = std::fs::read_to_string(root.join(MANIFEST))
        .map_err(|error| format!("{MANIFEST}: {error}"))?;
    let declared = manifest_version(&manifest)?;
    let (released, published) = released(root)?;
    if check_self {
        self_check(&released, &published)?;
    }
    let verdict = decision(
        &released,
        &published,
        candidate,
        declared_breakage(root, &declared)?,
    )?;
    println!(
        "{}",
        serde_json::json!({
            "current": verdict.current,
            "change": verdict.change.name(),
            "next": verdict.next,
            "removed": verdict.removed,
            "added": verdict.added,
        })
    );
    let history = versions_declared_in_history(root)?;
    println!(
        "{}",
        compare(
            &declared,
            &released,
            verdict.change,
            &verdict.next,
            &history
        )?
    );
    Ok(())
}
