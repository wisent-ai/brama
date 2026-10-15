//! The channels a baseline can come from, asked best first. A query answers
//! `Ok(None)` when its channel has nothing published, or when its program is
//! not installed on this machine (that channel is absent here), and an error
//! naming the command and its cause when the channel could not be asked: a
//! channel that could not be asked must not hand the baseline to a worse one.

use std::path::Path;
use std::process::{Command, ExitStatus};

use serde_json::Value;

use super::super::surface::of_revision;
use super::{holds_commit, PLATFORMS, PRODUCT, RELEASE_LIST_LIMIT, STADO_MARKER};

/// What a finished query printed, and the command line that printed it.
struct Answer {
    command: String,
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Answer {
    fn refused(&self) -> String {
        format!(
            "`{}` was refused ({}): {}",
            self.command,
            self.status,
            self.stderr.trim()
        )
    }

    /// Stdout of a query that succeeded; its refusal otherwise.
    fn succeeded(self) -> Result<String, String> {
        match self.status.success() {
            true => Ok(self.stdout),
            false => Err(self.refused()),
        }
    }

    fn json(&self) -> Result<Value, String> {
        serde_json::from_str(&self.stdout).map_err(|error| {
            format!(
                "`{}` did not answer JSON ({}): {error}; stderr: {}",
                self.command,
                self.status,
                self.stderr.trim()
            )
        })
    }
}

/// Run `program` in `root`. `Ok(None)` only when it is not installed here.
fn output(program: &str, root: &Path, arguments: &[&str]) -> Result<Option<Answer>, String> {
    let line = format!("{program} {}", arguments.join(" "));
    let mut command = Command::new(program);
    command.current_dir(root).args(arguments);
    match stado_wait::output(&mut command) {
        Ok(answer) => Ok(Some(Answer {
            command: line,
            status: answer.status,
            stdout: String::from_utf8_lossy(&answer.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&answer.stderr).into_owned(),
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("`{line}` could not be run: {error}")),
    }
}

/// The order of a `vMAJOR.MINOR.PATCH` tag (the `v` is optional); any other
/// tag is not a release and has none.
fn semver_order(tag: &str) -> Option<(u64, u64, u64)> {
    let number = match tag.strip_prefix('v') {
        Some(rest) => rest,
        None => tag,
    };
    let mut parts = number.split('.').map(str::parse::<u64>);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None) => Some((major, minor, patch)),
        _ => None,
    }
}

/// The newest Stado release whose source revision this checkout holds:
/// version, revision, artifact. `stado release status` exits non-zero when a
/// host is behind, which says nothing about the answer on stdout, so its
/// stdout is read whatever the exit status and only a reply that is not JSON
/// is refused.
pub(super) fn stado_release(root: &Path) -> Result<Option<(String, String, String)>, String> {
    // Unset is the `stado` on the search path; a value that cannot be read is
    // refused by name rather than read as unset.
    let stado = match brama::environment::declared_setting("STADO_BIN")? {
        Some(program) => program,
        None => String::from("stado"),
    };
    let Some(answer) = output(&stado, root, &["release", "status", PRODUCT, "--json"])? else {
        return Ok(None);
    };
    let report = answer.json()?;
    let mut published = Vec::new();
    for target in report["targets"].as_array().into_iter().flatten() {
        let Some(version) = target["desired"]["version"].as_str() else {
            continue;
        };
        let Some(order) = semver_order(version) else {
            continue;
        };
        for (platform, artifact) in target["desired"]["artifacts"]
            .as_object()
            .into_iter()
            .flatten()
        {
            let (Some(revision), Some(manifest)) = (
                artifact["source_revision"].as_str(),
                artifact["manifest_uri"].as_str(),
            ) else {
                continue;
            };
            if !revision.is_empty() && manifest.starts_with(STADO_MARKER) {
                published.push((
                    order,
                    version.to_string(),
                    revision.to_string(),
                    format!("{STADO_MARKER}{PRODUCT}/{version}/{platform}"),
                ));
            }
        }
    }
    published.sort();
    Ok(published
        .into_iter()
        .rev()
        .find(|(_, version, revision, _)| {
            let held = holds_commit(root, revision);
            if !held {
                eprintln!("published {version} names source revision {revision}, which this checkout does not hold; looking further back.");
            }
            held
        })
        .map(|(_, version, revision, artifact)| (version, revision, artifact)))
}

/// The newest non-draft GitHub Release whose tree declares its tag and which
/// carries every platform archive and checksum.
pub(super) fn github_release(root: &Path) -> Result<Option<String>, String> {
    let Some(answer) = output(
        "gh",
        root,
        &[
            "release",
            "list",
            "--limit",
            RELEASE_LIST_LIMIT,
            "--json",
            "tagName,isDraft,isPrerelease",
        ],
    )?
    else {
        return Ok(None);
    };
    if !answer.status.success() {
        return Err(answer.refused());
    }
    let list = answer.json()?;
    let mut tags: Vec<((u64, u64, u64), String)> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter(|release| release["isDraft"] == false && release["isPrerelease"] == false)
        .filter_map(|release| {
            let tag = release["tagName"].as_str()?;
            Some((semver_order(tag)?, tag.to_string()))
        })
        .collect();
    tags.sort();
    for (_, tag) in tags.into_iter().rev() {
        let claimed = tag.trim_start_matches('v');
        if of_revision(root, &tag)
            .map(|(_, declared)| declared)
            .as_deref()
            != Ok(claimed)
        {
            eprintln!("release {tag}: its tree declares another version; looking further back.");
            continue;
        }
        let Some(answer) = output("gh", root, &["release", "view", &tag, "--json", "assets"])?
        else {
            return Ok(None);
        };
        if !answer.status.success() {
            return Err(answer.refused());
        }
        let assets = answer.json()?;
        let names: Vec<&str> = assets["assets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|asset| asset["name"].as_str())
            .collect();
        let missing: Vec<String> = PLATFORMS
            .iter()
            .flat_map(|platform| {
                [
                    format!("{PRODUCT}-{tag}-{platform}.tar.gz"),
                    format!("{PRODUCT}-{tag}-{platform}.tar.gz.sha256"),
                ]
            })
            .filter(|name| !names.contains(&name.as_str()))
            .collect();
        if missing.is_empty() {
            return Ok(Some(tag));
        }
        eprintln!(
            "release {tag} is incomplete; missing {}; looking further back.",
            missing.join(", ")
        );
    }
    Ok(None)
}

/// The newest SemVer tag whose tree declares the tagged version.
pub(super) fn honest_tag(root: &Path) -> Result<Option<String>, String> {
    let Some(answer) = output("git", root, &["tag", "--list"])? else {
        return Ok(None);
    };
    let listing = answer.succeeded()?;
    let mut tags: Vec<((u64, u64, u64), String)> = listing
        .lines()
        .filter_map(|tag| Some((semver_order(tag.trim())?, tag.trim().to_string())))
        .collect();
    tags.sort();
    Ok(tags.into_iter().rev().map(|(_, tag)| tag).find(|tag| {
        of_revision(root, tag)
            .map(|(_, declared)| declared)
            .as_deref()
            == Ok(tag.trim_start_matches('v'))
    }))
}

/// The checkout's HEAD, the baseline when nothing is published.
pub(super) fn head(root: &Path) -> Result<String, String> {
    match output("git", root, &["rev-parse", "HEAD"])? {
        Some(answer) => Ok(answer.succeeded()?.trim().to_string()),
        None => Err("git is not installed here, so the checkout's HEAD cannot be read".into()),
    }
}
