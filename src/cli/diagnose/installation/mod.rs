//! What is installed here, and whether this host was told about that exact
//! copy. The units name a path, the path leads to a generation, and the trust
//! registry pins the uid, gid, absolute path and SHA-256 of the binary allowed
//! to redeem a capability; each is printed beside what it has to agree with.
//! A path that cannot be resolved or read is printed with its reason, never
//! left out of the comparison it belonged to.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::trust::{registry_verdict, router_answers, CAPABILITY_COMMAND};
use super::{moment, Layout};

// `say!` writes through `super::emit`, which from `env` and `units` is this
// module.
use super::emit;

mod env;
mod units;
pub(super) use env::print_service_env;
pub(super) use units::print_units;
use units::resolved_or_said;

/// The files a complete generation carries.
const REQUIRED_FILES: &[&str] = &[
    "bin/brama",
    "bin/skarbiec-entitlements-router",
    "bin/start-with-skarbiec",
    "bin/provision-skarbiec-trust",
    "libexec/generate-skarbiec-config.mjs",
    // The modules the generator imports; without them it dies on its
    // first line, so the generation is not complete.
    "libexec/skarbiec-config/keys.mjs",
    "libexec/skarbiec-config/requirement.mjs",
];
/// The programs a generation must be able to run.
const RUNNABLE_FILES: &[&str] = &[
    "bin/brama",
    "bin/skarbiec-entitlements-router",
    "bin/start-with-skarbiec",
];
/// The execute permission for owner, group and others, by its POSIX names.
const EXECUTE_BITS: u32 = (libc::S_IXUSR | libc::S_IXGRP | libc::S_IXOTH) as u32;

/// Every generation directory, oldest first by modification time; one whose
/// time cannot be read is said and listed after the rest.
fn installed_generations(layout: &Layout) -> Vec<PathBuf> {
    // A directory that is not there holds no generation; one that cannot be
    // listed, or an entry of it that cannot be, may hold the generation that
    // runs, so the diagnosis says so.
    let listing = |directory: &Path| -> Vec<PathBuf> {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => {
                say!("  {}: cannot be listed: {error}", directory.display());
                return Vec::new();
            }
        };
        let mut directories = Vec::new();
        for entry in entries {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    say!(
                        "  {}: an entry cannot be listed: {error}",
                        directory.display()
                    );
                    continue;
                }
            };
            if path.file_name().is_some_and(|name| name == "releases") {
                continue;
            }
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => directories.push(path),
                Ok(_file) => {}
                Err(error) => say!("  {}: cannot be inspected: {error}", path.display()),
            }
        }
        directories
    };
    let mut generations = listing(&layout.services);
    generations.extend(listing(&layout.services.join("releases")));
    let mut dated: Vec<_> = generations
        .into_iter()
        .map(
            |path| match std::fs::metadata(&path).and_then(|meta| meta.modified()) {
                Ok(modified) => (Some(modified), path),
                Err(error) => {
                    say!(
                        "  {}: modification time cannot be read: {error}",
                        path.display()
                    );
                    (None, path)
                }
            },
        )
        .collect();
    dated.sort_by(|(left_time, left), (right_time, right)| {
        (left_time.is_none(), left_time, left).cmp(&(right_time.is_none(), right_time, right))
    });
    dated.into_iter().map(|(_, path)| path).collect()
}

/// The roles the release state records, each with the release directory it
/// resolves to; a role whose directory does not resolve is said once.
fn recorded_roles(layout: &Layout) -> Vec<(String, PathBuf)> {
    let Some(state) = layout.release_state.as_object() else {
        say!("  the release state is not an object, so no generation carries a role");
        return Vec::new();
    };
    state
        .keys()
        .filter_map(|role| match layout.release_root(role) {
            Ok(root) => Some((role.clone(), root)),
            Err(reason) => {
                say!("  release role {role}: {reason}");
                None
            }
        })
        .collect()
}

/// Whether a file's mode grants nobody the right to execute it.
fn executable_by_nobody(mode: u32) -> bool {
    std::num::NonZeroU32::new(mode & EXECUTE_BITS).is_none()
}

pub(super) fn print_generations(layout: &Layout, resolved: Option<&Path>) {
    say!("\n=== installed generations");
    let roles_recorded = recorded_roles(layout);
    for generation in installed_generations(layout) {
        let root = match crate::cli::generation::generation_root(&generation) {
            Ok(root) => root,
            Err(reason) => {
                say!("  {}: not a generation: {reason}", generation.display());
                continue;
            }
        };
        let mut roles: Vec<String> = match resolved_or_said(&root, "generation") {
            Some(canonical) => roles_recorded
                .iter()
                .filter(|(_, release_root)| *release_root == canonical)
                .map(|(role, _)| role.clone())
                .collect(),
            None => Vec::new(),
        };
        if resolved.is_some() && resolved_or_said(&generation, "generation").as_deref() == resolved
        {
            roles.push("legacy current".into());
        }
        let marker = if roles.is_empty() {
            String::new()
        } else {
            format!(" <- {}", roles.join(", "))
        };
        // A required file whose presence cannot be checked is said with the
        // reason, never counted as present or as missing.
        let mut missing: Vec<&str> = Vec::new();
        for name in REQUIRED_FILES.iter().copied() {
            match root.join(name).try_exists() {
                Ok(true) => {}
                Ok(false) => missing.push(name),
                Err(error) => say!("    {name}: presence cannot be checked: {error}"),
            }
        }
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
        let mut unrunnable: Vec<&str> = Vec::new();
        for name in RUNNABLE_FILES.iter().copied() {
            match std::fs::metadata(root.join(name)) {
                Ok(meta) if meta.is_file() && executable_by_nobody(meta.permissions().mode()) => {
                    unrunnable.push(name)
                }
                Ok(_runnable) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => say!("    {name}: cannot be inspected: {error}"),
            }
        }
        if !unrunnable.is_empty() {
            say!("    NOT EXECUTABLE: {}", unrunnable.join(", "));
        }
        match generation.file_name() {
            Some(name) => {
                for line in registry_verdict(&root, &layout.config_dir_for(name)) {
                    say!("    registry: {line}");
                }
            }
            None => say!(
                "    registry: {} names no generation directory",
                generation.display()
            ),
        }
    }
}
