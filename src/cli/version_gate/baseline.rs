//! `released-surface.json`: the surface of the release every change is
//! measured against. It is generated from the best channel that published
//! Brama, best first, and checked against that channel:
//!
//! - `stado://releases/…`: the Stado release plane, which production installs
//!   from; the recorded source revision's surface is recomputed and its
//!   version must be one this tree declares or rolls back to;
//! - `github-release:<tag>`: a GitHub Release carrying every archive and
//!   checksum; it must still be the best complete one;
//! - `git-archive:<tag>`: a SemVer tag whose tree declares the tagged version;
//! - `head:<sha>`: nothing is published; the commit's surface must match.
//!
//! A channel is passed over only when it has nothing published or its program
//! is not installed here; a channel that could not be asked (a refused `gh`,
//! a `stado` reply that is not JSON) stops the baseline with that cause.

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

use super::contract::{read_json, BASELINE};
use super::surface::{manifest_version, of_revision};

mod channels;
const PRODUCT: &str = "brama";
const STADO_MARKER: &str = "stado://releases/";
const GITHUB_MARKER: &str = "github-release:";
const ARCHIVE_MARKER: &str = "git-archive:";
const HEAD_MARKER: &str = "head:";
const PLATFORMS: [&str; 2] = ["linux-amd64", "darwin-arm64"];
const RELEASE_LIST_LIMIT: &str = "100";

fn holds_commit(root: &Path, revision: &str) -> bool {
    Command::new("git")
        .current_dir(root)
        .args(["cat-file", "-e", &format!("{revision}^{{commit}}")])
        .status()
        .is_ok_and(|status| status.success())
}

/// The baseline document from the best channel. A channel that could not be
/// asked stops here with its cause rather than handing the baseline to a
/// worse channel.
pub(super) fn best(root: &Path) -> Result<Value, String> {
    let (reference, source) = if let Some((_, revision, artifact)) = channels::stado_release(root)?
    {
        (revision.clone(), format!("{artifact} source_revision={revision} -- signed release object the Stado release plane published and hosts install from"))
    } else if let Some(tag) = channels::github_release(root)? {
        (tag.clone(), format!("{GITHUB_MARKER}{tag} -- immutable archives and checksums published by the repository release workflow"))
    } else if let Some(tag) = channels::honest_tag(root)? {
        (tag.clone(), format!("{ARCHIVE_MARKER}{tag} -- no complete GitHub Release exists; this tag is the best recoverable source baseline"))
    } else {
        let head = channels::head(root)?;
        (head.clone(), format!("{HEAD_MARKER}{head} -- NOT PUBLISHED: no complete GitHub Release or usable SemVer tag exists"))
    };
    let (surface, declared) = of_revision(root, &reference)?;
    Ok(json!({"version": declared, "source": source, "surface": surface}))
}

/// Check the committed baseline against the channel that published it.
pub(super) fn check(root: &Path) -> Result<String, String> {
    let baseline = read_json(&root.join(BASELINE))?;
    let source = baseline["source"].as_str().unwrap_or("");
    let version = baseline["version"].as_str().unwrap_or("");
    let surface: Vec<String> = baseline["surface"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if source.is_empty() || version.is_empty() || surface.is_empty() {
        return Err(format!("{BASELINE} must carry source, version and surface"));
    }
    let regenerate = "regenerate it with brama version-gate baseline --write";
    let marker = source.split(' ').next().unwrap_or("");
    if source.starts_with(STADO_MARKER) {
        let revision = source
            .split("source_revision=")
            .nth(1)
            .and_then(|rest| rest.split(' ').next())
            .filter(|revision| !revision.is_empty())
            .ok_or("a stado:// baseline must record source_revision=<sha> in its source")?;
        if !holds_commit(root, revision) {
            return Err(format!("the baseline names source revision {revision}, which this checkout does not hold; fetch it or {regenerate}"));
        }
        let (recomputed, declared) = of_revision(root, revision)?;
        if declared != version {
            return Err(format!(
                "the baseline records version {version}, but {revision} declares {declared}"
            ));
        }
        if recomputed != surface {
            return Err(format!(
                "the baseline surface does not match the surface of {revision}; {regenerate}"
            ));
        }
        let release = read_json(&root.join(".wisent-release.json"))?;
        let manifest =
            std::fs::read_to_string(root.join("Cargo.toml")).map_err(|error| error.to_string())?;
        let known = release["runtime"]["rollback_compatible_with"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|entry| entry.as_str() == Some(version));
        if !known && manifest_version(&manifest)? != version {
            return Err(format!("the baseline records {version}, which this tree neither declares nor lists under runtime.rollback_compatible_with"));
        }
        return Ok(format!(
            "Baseline {version} matches the Stado release plane and the surface of {revision}."
        ));
    }
    if source.starts_with(GITHUB_MARKER) {
        let best = best(root)?;
        let want = best["source"]
            .as_str()
            .unwrap_or("")
            .split(' ')
            .next()
            .unwrap_or("")
            .to_string();
        if marker != want {
            return Err(format!(
                "the baseline is {marker}, but {want} is the best complete release; {regenerate}"
            ));
        }
        return Ok(format!("Baseline {marker} matches the release channel."));
    }
    if let Some(reference) = marker
        .strip_prefix(ARCHIVE_MARKER)
        .or(marker.strip_prefix(HEAD_MARKER))
    {
        let (recomputed, declared) = of_revision(root, reference).map_err(|_| {
            format!("the baseline names {reference}, which this checkout cannot read")
        })?;
        if declared != version || recomputed != surface {
            return Err(format!(
                "the baseline does not match {reference}; {regenerate}"
            ));
        }
        return Ok(format!("Baseline {version} matches {reference}."));
    }
    Err(format!("unknown baseline marker: {marker}"))
}
