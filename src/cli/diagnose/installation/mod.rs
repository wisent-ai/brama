//! What is installed here, and whether this host was told about that exact
//! copy. The units name a path, the path leads to a generation, and the trust
//! registry pins the uid, gid, absolute path and SHA-256 of the binary allowed
//! to redeem a capability; each is printed beside what it has to agree with.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use super::trust::{registry_verdict, router_answers, CAPABILITY_COMMAND};
use super::{moment, Layout, SERVICE_LABEL};

// `say!` writes through `super::emit`, which from `env` is this module.
use super::emit;

mod env;
pub(super) use env::print_service_env;

/// The files a complete generation carries.
const REQUIRED_FILES: [&str; 5] = [
    "bin/brama",
    "bin/skarbiec-entitlements-router",
    "bin/start-with-skarbiec",
    "bin/provision-skarbiec-trust",
    "libexec/generate-skarbiec-config.mjs",
];
/// The programs a generation must be able to run.
const RUNNABLE_FILES: [&str; 3] = [
    "bin/brama",
    "bin/skarbiec-entitlements-router",
    "bin/start-with-skarbiec",
];

/// A launchd plist as JSON, through the system's own converter; why it
/// could not be converted otherwise.
fn plist(path: &Path) -> Result<Value, String> {
    let mut command = Command::new("/usr/bin/plutil");
    command.args(["-convert", "json", "-o", "-"]).arg(path);
    let output = stado_wait::output(&mut command)
        .map_err(|error| format!("plutil could not be run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "plutil refused it ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| format!("plutil's JSON: {error}"))
}

/// The units that start Brama; returns where `current` resolves.
pub(super) fn print_units(layout: &Layout) -> Option<PathBuf> {
    say!("=== units that start Brama");
    let current = layout.services.join("current");
    let resolved = std::fs::canonicalize(&current).ok();
    if let Ok(target) = std::fs::read_link(&current) {
        say!(
            "current -> {} (link written {})",
            target.display(),
            moment(&current)
        );
    }
    for location in [
        PathBuf::from("/Library/LaunchDaemons"),
        layout.home.join("Library/LaunchAgents"),
    ] {
        // A directory that is not there holds no unit; one that cannot be
        // listed may hold the unit that runs Brama, so the diagnosis says so.
        let entries = match std::fs::read_dir(&location) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                say!("  {}: cannot be listed: {error}", location.display());
                continue;
            }
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "plist"))
            .collect();
        paths.sort();
        for path in paths {
            // A unit that cannot be read may be the one that runs Brama.
            let document = match plist(&path) {
                Ok(document) => document,
                Err(cause) => {
                    say!("  {}: cannot be read: {cause}", path.display());
                    continue;
                }
            };
            let label = match document.get("Label").and_then(Value::as_str) {
                Some(label) => label,
                None => "(no Label)",
            };
            let arguments: Vec<&str> = document
                .get("ProgramArguments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let joined = arguments.join(" ");
            if !label.contains("brama") && !joined.contains("start-with-skarbiec") {
                continue;
            }
            let verdict = if label == SERVICE_LABEL {
                "the one unit"
            } else {
                "DEFECT: a second unit; only com.wisent.brama runs Brama, and Stado removes this one with the next release"
            };
            say!(
                "  {}\n    label:    {label} ({verdict})\n    program:  {joined}",
                path.display()
            );
            if let Some(file) = arguments
                .iter()
                .map(Path::new)
                .find(|candidate| candidate.is_file())
            {
                say!(
                    "    leads to: {}",
                    std::fs::canonicalize(file)
                        .unwrap_or(file.to_path_buf())
                        .display()
                );
            }
        }
    }
    resolved
}

fn installed_generations(layout: &Layout) -> Vec<PathBuf> {
    // A directory that is not there holds no generation; one that cannot be
    // listed may hold the generation that runs, so the diagnosis says so.
    let listing = |directory: &Path| -> Vec<PathBuf> {
        match std::fs::read_dir(directory) {
            Ok(entries) => entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.file_name().is_some_and(|name| name != "releases"))
                .filter(|path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()))
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                say!("  {}: cannot be listed: {error}", directory.display());
                Vec::new()
            }
        }
    };
    let mut generations = listing(&layout.services);
    generations.extend(listing(&layout.services.join("releases")));
    generations.sort_by_key(|path| {
        (
            std::fs::metadata(path)
                .and_then(|meta| meta.modified())
                .ok(),
            path.clone(),
        )
    });
    generations
}

fn architecture_root(generation: &Path) -> PathBuf {
    ["darwin-arm64", "darwin-arm"]
        .iter()
        .map(|platform| generation.join(platform))
        .find(|path| path.is_dir())
        .unwrap_or(generation.to_path_buf())
}

pub(super) fn print_generations(layout: &Layout, resolved: Option<&Path>) {
    say!("\n=== installed generations");
    // Every role the release state records, as it records them.
    let roles_recorded: Vec<String> = layout
        .release_state
        .as_object()
        .map(|state| state.keys().cloned().collect())
        .unwrap_or_default();
    for generation in installed_generations(layout) {
        let root = architecture_root(&generation);
        if !root.join("bin").is_dir() {
            continue;
        }
        let canonical = std::fs::canonicalize(&root).ok();
        let mut roles: Vec<String> = roles_recorded
            .iter()
            .filter(|role| canonical.is_some() && layout.release_root(role) == canonical)
            .cloned()
            .collect();
        if std::fs::canonicalize(&generation).ok().as_deref() == resolved {
            roles.push("legacy current".into());
        }
        let marker = if roles.is_empty() {
            String::new()
        } else {
            format!(" <- {}", roles.join(", "))
        };
        let missing: Vec<&str> = REQUIRED_FILES
            .iter()
            .copied()
            .filter(|name| !root.join(name).exists())
            .collect();
        say!(
            "  {}{marker}  installed {}",
            generation.display(),
            moment(&generation)
        );
        say!(
            "    files:    {}",
            if missing.is_empty() {
                "complete".into()
            } else {
                format!("missing {}", missing.join(", "))
            }
        );
        say!(
            "    router {}: {}",
            CAPABILITY_COMMAND.join(" "),
            router_answers(&root)
        );
        let unrunnable: Vec<&str> = RUNNABLE_FILES
            .iter()
            .copied()
            .filter(|name| {
                std::fs::metadata(root.join(name))
                    .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 == 0)
            })
            .collect();
        if !unrunnable.is_empty() {
            say!("    NOT EXECUTABLE: {}", unrunnable.join(", "));
        }
        for line in registry_verdict(&root, &layout.config_dir_for(&generation)) {
            say!("    registry: {line}");
        }
    }
}
