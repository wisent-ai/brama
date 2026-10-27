//! The launchd units that start Brama, and where `current` resolves.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use super::super::{moment, Layout, SERVICE_LABEL};

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

/// Where a path resolves, or `None` after the diagnosis says why it does not.
pub(super) fn resolved_or_said(path: &Path, what: &str) -> Option<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(resolved) => Some(resolved),
        Err(error) => {
            say!("  {what} {}: cannot be resolved: {error}", path.display());
            None
        }
    }
}

/// The units that start Brama; returns where `current` resolves. A unit is
/// Brama's when it is the declared service label or when a program argument
/// lies inside Brama's installed generations, whatever its label says.
pub(in super::super) fn print_units(layout: &Layout) -> Option<PathBuf> {
    say!("=== units that start Brama");
    let current = layout.services.join("current");
    let resolved = match std::fs::symlink_metadata(&current) {
        Ok(_) => resolved_or_said(&current, "current"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            say!("  current {}: cannot be read: {error}", current.display());
            None
        }
    };
    if let Ok(target) = std::fs::read_link(&current) {
        say!(
            "current -> {} (link written {})",
            target.display(),
            moment(&current)
        );
    }
    let services = resolved_or_said(&layout.services, "the services directory");
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
        // An entry the directory cannot list may be the unit that runs
        // Brama, so the diagnosis says it rather than skipping it.
        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => paths.push(entry.path()),
                Err(error) => say!("  {}: an entry cannot be listed: {error}", location.display()),
            }
        }
        paths.retain(|path| path.extension().is_some_and(|ext| ext == "plist"));
        paths.sort();
        for path in paths {
            print_unit(&path, services.as_deref());
        }
    }
    resolved
}

/// One unit, when it is Brama's: its label, its program and where the
/// program leads.
fn print_unit(path: &Path, services: Option<&Path>) {
    // A unit that cannot be read may be the one that runs Brama.
    let document = match plist(path) {
        Ok(document) => document,
        Err(cause) => {
            say!("  {}: cannot be read: {cause}", path.display());
            return;
        }
    };
    let label = document.get("Label").and_then(Value::as_str);
    let arguments: Vec<&str> = document
        .get("ProgramArguments")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let runs_a_generation = services.is_some_and(|services| {
        arguments.iter().any(|argument| {
            std::fs::canonicalize(argument).is_ok_and(|resolved| resolved.starts_with(services))
        })
    });
    if label != Some(SERVICE_LABEL) && !runs_a_generation {
        return;
    }
    let verdict = if label == Some(SERVICE_LABEL) {
        "the one unit"
    } else {
        "DEFECT: a second unit; only com.wisent.brama runs Brama, and Stado removes this one with the next release"
    };
    let label = match label {
        Some(label) => label.to_string(),
        None => "(no Label)".to_string(),
    };
    say!(
        "  {}\n    label:    {label} ({verdict})\n    program:  {}",
        path.display(),
        arguments.join(" ")
    );
    // The first argument that is a file is the program the unit runs; an
    // absolute argument that cannot be examined is said with its cause,
    // since it may be that program, and passing over it hid the answer.
    for argument in arguments.iter().map(Path::new) {
        match std::fs::metadata(argument) {
            Ok(metadata) if metadata.is_file() => {
                match std::fs::canonicalize(argument) {
                    Ok(target) => say!("    leads to: {}", target.display()),
                    Err(error) => say!(
                        "    leads to: {} (cannot be resolved: {error})",
                        argument.display()
                    ),
                }
                break;
            }
            Ok(_not_file) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) if argument.is_absolute() => {
                say!("    {} cannot be examined: {error}", argument.display())
            }
            Err(_not_a_path) => {}
        }
    }
}
