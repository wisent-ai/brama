//! The service env beside the runtime coordinates, and the trust config
//! directory of the generation that serves.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::super::Layout;

/// A release-record field as the record states it; an absent field is said
/// to be absent rather than shown as an empty value.
fn field(record: &Value, key: &str) -> String {
    match record.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => "absent".into(),
    }
}

/// The generation directory a legacy `current` resolves into: the parent of
/// its `darwin-*` architecture tree when it resolves to one.
fn legacy_generation(path: &Path) -> PathBuf {
    let darwin = path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("darwin-"));
    match (darwin, path.parent()) {
        (true, Some(parent)) => parent.to_path_buf(),
        _ => path.to_path_buf(),
    }
}

/// The trust config and runtime directories the active release names; none,
/// said why, when its release directory cannot be resolved to a generation.
fn active(layout: &Layout, record: &Value) -> Option<(PathBuf, PathBuf)> {
    let root = match layout.release_root("active") {
        Ok(root) => root,
        Err(cause) => {
            say!(
                "active release: {} release_dir={} does not resolve on this host: {cause}",
                field(record, "version"),
                field(record, "release_dir")
            );
            return None;
        }
    };
    say!(
        "active release: {} pid={} port={} root={}",
        field(record, "version"),
        field(record, "pid"),
        field(record, "port"),
        root.display()
    );
    let Some(generation) = root.parent().and_then(Path::file_name) else {
        say!(
            "active release root {} has no generation directory above it",
            root.display()
        );
        return None;
    };
    let runtime = layout.home.join(".stado/run/brama").join(format!(
        "{}-{}",
        field(record, "version"),
        field(record, "port")
    ));
    Some((layout.config_dir_for(generation), runtime))
}

/// The trust config and runtime directories a legacy `current` names, or the
/// pre-generation layout when nothing is current.
fn legacy(layout: &Layout, resolved: Option<&Path>) -> Option<(PathBuf, PathBuf)> {
    say!("active release: absent; inspecting legacy current");
    let Some(resolved) = resolved else {
        return Some((
            layout.home.join(".config/brama/trust"),
            layout.home.join(".stado/run/brama-skarbiec"),
        ));
    };
    let generation = legacy_generation(resolved);
    let Some(name) = generation.file_name() else {
        say!(
            "legacy current {} names no generation",
            generation.display()
        );
        return None;
    };
    let runtime = layout
        .home
        .join(".stado/run")
        .join(format!("brama-skarbiec-{}", name.to_string_lossy()));
    Some((layout.config_dir_for(name), runtime))
}

/// The service env beside the runtime coordinates; returns the trust config
/// directory of the generation that serves, or none when the active release
/// names one that cannot be found. A value is shown only when it is an
/// existing path, so no secret the env carries is ever printed; a value
/// that cannot be examined is not shown either, but the cause is, so a path
/// the service cannot reach is not read as "not a path".
pub(in crate::cli::diagnose) fn print_service_env(
    layout: &Layout,
    resolved: Option<&Path>,
) -> Option<PathBuf> {
    say!("\n=== service env");
    for (name, value) in &layout.settings {
        match std::fs::metadata(value) {
            Ok(_metadata) => say!("  {name}={value}"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                say!("  {name}=<not a path; value not shown>")
            }
            Err(error) => say!("  {name}=<cannot be examined: {error}; value not shown>"),
        }
    }
    let (config_dir, runtime_dir) = match layout.release_record("active") {
        Some(record) => active(layout, record)?,
        None => legacy(layout, resolved)?,
    };
    say!("config dir:  {}", config_dir.display());
    say!("runtime dir: {}", runtime_dir.display());
    Some(config_dir)
}
