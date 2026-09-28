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

/// A launchd plist as JSON, through the system's own converter.
fn plist(path: &Path) -> Option<Value> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-"])
        .arg(path)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| serde_json::from_slice(&output.stdout).ok())
        .flatten()
}

/// The units that start Brama; returns where `current` resolves.
pub(super) fn print_units(layout: &Layout) -> Option<PathBuf> {
    println!("=== units that start Brama");
    let current = layout.services.join("current");
    let resolved = std::fs::canonicalize(&current).ok();
    if let Ok(target) = std::fs::read_link(&current) {
        println!(
            "current -> {} (link written {})",
            target.display(),
            moment(&current)
        );
    }
    for location in [
        PathBuf::from("/Library/LaunchDaemons"),
        layout.home.join("Library/LaunchAgents"),
    ] {
        let Ok(entries) = std::fs::read_dir(&location) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "plist"))
            .collect();
        paths.sort();
        for path in paths {
            let Some(document) = plist(&path) else {
                continue;
            };
            let label = document.get("Label").and_then(Value::as_str).unwrap_or("");
            let arguments: Vec<&str> = document
                .get("ProgramArguments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let joined = arguments.join(" ");
            if !label.contains(SERVICE_LABEL) && !joined.contains("start-with-skarbiec") {
                continue;
            }
            println!(
                "  {}\n    label:    {label}\n    program:  {joined}",
                path.display()
            );
            if let Some(file) = arguments
                .iter()
                .map(Path::new)
                .find(|candidate| candidate.is_file())
            {
                println!(
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
    let listing = |directory: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(directory)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.file_name().is_some_and(|name| name != "releases"))
                    .filter(|path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()))
                    .collect()
            })
            .unwrap_or_default()
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
    println!("\n=== installed generations");
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
        println!(
            "  {}{marker}  installed {}",
            generation.display(),
            moment(&generation)
        );
        println!(
            "    files:    {}",
            if missing.is_empty() {
                "complete".into()
            } else {
                format!("missing {}", missing.join(", "))
            }
        );
        println!(
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
            println!("    NOT EXECUTABLE: {}", unrunnable.join(", "));
        }
        for line in registry_verdict(&root, &layout.config_dir_for(&generation)) {
            println!("    registry: {line}");
        }
    }
}

/// The service env beside the runtime coordinates; returns the trust config
/// directory of the generation that serves. A value is shown only when it is
/// an existing path, so no secret the env carries is ever printed.
pub(super) fn print_service_env(layout: &Layout, resolved: Option<&Path>) -> PathBuf {
    println!("\n=== service env");
    for (name, value) in &layout.settings {
        if Path::new(value).exists() {
            println!("  {name}={value}");
        } else {
            println!("  {name}=<not a path; value not shown>");
        }
    }
    let (config_dir, runtime_dir) = match layout.release_record("active") {
        Some(record) => {
            let text = |key: &str| {
                record
                    .get(key)
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| value.to_string())
                    })
                    .unwrap_or_else(|| "None".into())
            };
            let root = layout.release_root("active").unwrap_or_default();
            let generation = root.parent().map(Path::to_path_buf).unwrap_or_default();
            println!(
                "active release: {} pid={} port={} root={}",
                text("version"),
                text("pid"),
                text("port"),
                root.display()
            );
            let runtime = layout.home.join(".stado/run/brama").join(format!(
                "{}-{}",
                text("version"),
                text("port")
            ));
            (layout.config_dir_for(&generation), runtime)
        }
        None => {
            let generation = resolved.map(|path| {
                let darwin = path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("darwin-"));
                if darwin {
                    path.parent().unwrap_or(path).to_path_buf()
                } else {
                    path.to_path_buf()
                }
            });
            println!("active release: absent; inspecting legacy current");
            match generation {
                Some(generation) => {
                    let name = generation
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    (
                        layout.config_dir_for(&generation),
                        layout
                            .home
                            .join(".stado/run")
                            .join(format!("brama-skarbiec-{name}")),
                    )
                }
                None => (
                    layout.home.join(".config/brama/trust"),
                    layout.home.join(".stado/run/brama-skarbiec"),
                ),
            }
        }
    };
    println!("config dir:  {}", config_dir.display());
    println!("runtime dir: {}", runtime_dir.display());
    config_dir
}
