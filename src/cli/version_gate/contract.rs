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

pub(super) fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

/// A `MAJOR.MINOR.PATCH` version, the only form the gate compares.
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Version {
    fn parse(version: &str) -> Result<Self, String> {
        let mut parts = version.split('.');
        let (Some(major), Some(minor), Some(patch), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(format!("{version} is not MAJOR.MINOR.PATCH"));
        };
        let number = |part: &str| {
            part.parse::<u64>()
                .map_err(|error| format!("{version} is not MAJOR.MINOR.PATCH: `{part}`: {error}"))
        };
        Ok(Self {
            major: number(major)?,
            minor: number(minor)?,
            patch: number(patch)?,
        })
    }

    /// This series at `patch`.
    fn at_patch(&self, patch: u64) -> String {
        format!("{}.{}.{patch}", self.major, self.minor)
    }
}

/// The surface list of a surface document (`what` names it: the committed
/// baseline or a candidate file): refused when it is absent or empty, or
/// when an entry is not text. Read with odd entries dropped, the gate
/// compared a shortened surface and could miss a removed name.
pub(super) fn surface_list(document: &Value, what: &str) -> Result<Vec<String>, String> {
    document["surface"]
        .as_array()
        .filter(|entries| !entries.is_empty())
        .ok_or_else(|| format!("{what} carries no surface list: {}", document["surface"]))?
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{what} lists a surface entry that is not text: {entry}"))
        })
        .collect()
}

/// The baseline's version and surface.
pub(super) fn released(root: &Path) -> Result<(String, Vec<String>), String> {
    let baseline = read_json(&root.join(BASELINE))?;
    let version = baseline["version"]
        .as_str()
        .filter(|version| !version.is_empty())
        .ok_or(format!("{BASELINE} names no released version"))?;
    Ok((version.to_string(), surface_list(&baseline, BASELINE)?))
}

/// Whether the owner declared, for this very version, a break the surface
/// cannot show. It expires when the manifest moves past that version.
fn declared_breakage(root: &Path, declared: &str) -> Result<bool, String> {
    let path = root.join(BREAKAGE);
    // Only a declaration that does not exist is no declaration: one that
    // cannot be examined would otherwise hide a declared break.
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(format!("{BREAKAGE} exists but is not a file")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("{BREAKAGE} cannot be examined: {error}")),
    }
    let declaration = read_json(&path)?;
    let reason = declaration["reason"].as_str().map(str::trim);
    if matches!(reason, None | Some("")) {
        return Err(format!(
            "{BREAKAGE} names no reason, so nothing here says what broke"
        ));
    }
    Ok(declaration["version"].as_str() == Some(declared))
}

/// Every version an earlier commit on this branch committed to Cargo.toml.
/// A commit whose manifest cannot be shown or names no version is refused
/// by name: read with such commits dropped, a skipped coordinate they
/// declared was reported as never declared.
fn versions_declared_in_history(root: &Path) -> Result<BTreeSet<String>, String> {
    let output = stado_wait::output(
        Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["log", "--format=%H", "--", MANIFEST]),
    )
    .map_err(|error| format!("git log -- {MANIFEST}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git log -- {MANIFEST} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let commits: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let earlier = match commits.split_first() {
        Some((_, earlier)) if !earlier.is_empty() => earlier,
        _ => {
            return Err("the manifest history is one commit deep; fetch the full history before comparing versions".into())
        }
    };
    earlier
        .iter()
        .map(|commit| {
            let text = git_show(root, commit, MANIFEST)
                .map_err(|error| format!("commit {commit}: {error}"))?;
            manifest_version(&text).map_err(|error| format!("commit {commit}: {error}"))
        })
        .collect()
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
    let (want, have) = (Version::parse(required)?, Version::parse(declared)?);
    if (have.major, have.minor) != (want.major, want.minor) || have.patch <= want.patch {
        return Err(format!("Cargo.toml commits {declared}, but the {class} change from {released} requires {required}."));
    }
    // The first coordinate nothing consumed refuses; the range is walked
    // lazily, so a jump of any size costs one coordinate past the last held.
    if let Some(coordinate) = (want.patch..have.patch)
        .map(|patch| want.at_patch(patch))
        .find(|coordinate| !history.contains(coordinate))
    {
        return Err(format!("Cargo.toml commits {declared}, but the {class} change from {released} requires {required}, and {coordinate} was never declared by an earlier commit on this branch, so nothing consumed it."));
    }
    let skipped: Vec<String> = (want.patch..have.patch).map(|patch| want.at_patch(patch)).collect();
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

/// Prove the rule and the comparison can both refuse before trusting either:
/// an unchanged surface is internal, the surface less its first command is
/// breaking, and the furthest patch of the required series is refused when
/// no earlier commit consumed the coordinates before it.
fn self_check(released: &str, published: &[String]) -> Result<(), String> {
    let unchanged = decision(released, published, published, false)?;
    let (_, shortened) = published
        .split_first()
        .ok_or(format!("{BASELINE} lists no command to remove in the self-check"))?;
    let broken = decision(released, published, shortened, false)?.change;
    if unchanged.change != Change::Internal || broken != Change::Breaking {
        return Err(format!(
            "self-check returned identical={} removed={}",
            unchanged.change.name(),
            broken.name()
        ));
    }
    let required = unchanged.next;
    let far = Version::parse(&required)?.at_patch(u64::MAX);
    if let Ok(accepted) = compare(&far, released, Change::Internal, &required, &BTreeSet::new()) {
        return Err(format!("the comparison accepted {far} against a required {required} with nothing consuming the gap: {accepted}"));
    }
    Ok(())
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
