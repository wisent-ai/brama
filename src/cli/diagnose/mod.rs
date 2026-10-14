//! `brama diagnose`: answer, on one host, why this gateway is or is not
//! serving.
//!
//! Every failure a host has had reads the same from outside — `/health`
//! answers and no route works, or nothing listens — while the cause was a
//! different file each time. So this opens all of them, in the order the
//! launcher does, and prints what each says beside what it has to agree with:
//!
//! 0. `unreadable inputs`, only when service.env or the release state could
//!    not be read: each with its cause, so a later section that finds no
//!    setting or no release record is not taken for a host without one;
//! 1. the units that start Brama (a plist that cannot be read is named with
//!    plutil's cause), and the release state that actually serves;
//! 2. every installed generation: completeness, router verbs, and whether the
//!    trust registry describes that exact installation;
//! 3. the service env beside the supervisor-owned runtime coordinates;
//! 4. the policy's provider grants against the authority-owned routes table;
//! 5. every alias route against the providers policy and routes agree on;
//! 6. where the gateway is reachable, and by which scheme;
//! 7. the current boot attempt, whole, from the launcher's first line on.
//!
//! Read-only throughout. Every section writes its lines through `say!`: on a
//! terminal they are printed as they are found, and with `--json` the same
//! lines are kept and printed once as `{sections: [{title, lines}]}`.

/// The lines `--json` keeps instead of printing; `None` prints them.
static COLLECTED: std::sync::Mutex<Option<Vec<String>>> = std::sync::Mutex::new(None);

/// One diagnosis line, printed now or kept for the JSON document.
pub(super) fn emit(line: String) {
    let mut collected = COLLECTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match collected.as_mut() {
        Some(lines) => lines.push(line),
        None => println!("{line}"),
    }
}

macro_rules! say {
    ($($arg:tt)*) => { super::emit(format!($($arg)*)) };
}

mod capability;
mod installation;
mod reachability;
mod trust;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::workload::service_settings;

/// The one unit Brama runs under on every host, as the Stado catalog names it.
pub(super) const SERVICE_LABEL: &str = "com.wisent.brama";

/// Where this host keeps Brama, read once and handed to every section.
pub(super) struct Layout {
    home: PathBuf,
    services: PathBuf,
    settings: BTreeMap<String, String>,
    release_state: Value,
    /// The unit's one log: Stado writes a unit's stdout and stderr to
    /// `~/.stado/logs/<unit>.log`.
    log: PathBuf,
    /// Inputs that could not be read (service.env missing or unreadable, a
    /// release state present but unreadable), with their causes: printed
    /// before every section, so a section that finds no setting or no release
    /// record is not read as the host having none.
    unreadable: Vec<String>,
}

impl Layout {
    fn read() -> Result<Self, String> {
        let home = super::workload::home()?;
        let mut unreadable = Vec::new();
        let settings = match service_settings(&home) {
            Ok(settings) => settings,
            Err(cause) => {
                unreadable.push(format!("service settings {cause}"));
                BTreeMap::new()
            }
        };
        let state_path = home.join(".stado/release-state/brama.json");
        let release_state = match read_json(&state_path) {
            Ok(Some(state)) => state,
            Ok(None) => Value::Null,
            Err(cause) => {
                unreadable.push(format!("release state {} {cause}", state_path.display()));
                Value::Null
            }
        };
        Ok(Layout {
            services: home.join(".stado/services/brama"),
            log: home
                .join(".stado/logs")
                .join(format!("{SERVICE_LABEL}.log")),
            home,
            settings,
            release_state,
            unreadable,
        })
    }

    /// One release-state record that names a release directory.
    fn release_record(&self, name: &str) -> Option<&Value> {
        self.release_state.get(name).filter(|record| {
            record
                .get("release_dir")
                .and_then(Value::as_str)
                .is_some_and(|dir| !dir.is_empty())
        })
    }

    /// The release directory a release-state record names, resolved on this
    /// host, or why it is not: no record naming one, or the resolution error.
    fn release_root(&self, name: &str) -> Result<PathBuf, String> {
        let dir = self
            .release_record(name)
            .and_then(|record| record.get("release_dir"))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("the release state names no {name} release_dir"))?;
        std::fs::canonicalize(dir).map_err(|error| format!("{dir}: {error}"))
    }

    /// The trust config directory of the generation with this directory name.
    fn config_dir_for(&self, generation: &std::ffi::OsStr) -> PathBuf {
        self.home
            .join(".config/brama")
            .join(format!("trust-{}", generation.to_string_lossy()))
    }
}

/// Seconds since the epoch as `YYYY-MM-DDTHH:MM:SSZ`, or why the file's
/// modification time could not be read; "unknown" dropped the cause.
pub(super) fn moment(path: &Path) -> String {
    match std::fs::symlink_metadata(path).and_then(|meta| meta.modified()) {
        Ok(time) => chrono::DateTime::<chrono::Utc>::from(time)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string(),
        Err(error) => format!("not read ({error})"),
    }
}

/// The JSON at `path`: `Ok(None)` only when there is no file, the read or
/// parse failure as the error otherwise, so a diagnosis never calls a file
/// absent that is present and broken.
pub(super) fn read_json(path: &Path) -> Result<Option<Value>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("not JSON: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("cannot be read: {error}")),
    }
}

pub(crate) async fn run(json: bool) {
    if json {
        *COLLECTED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Vec::new());
    }
    match Layout::read() {
        Ok(layout) => {
            if !layout.unreadable.is_empty() {
                emit(format!(
                    "=== unreadable inputs\n{}",
                    layout.unreadable.join("\n")
                ));
            }
            let resolved = installation::print_units(&layout);
            installation::print_generations(&layout, resolved.as_deref());
            match installation::print_service_env(&layout, resolved.as_deref()) {
                Some(config_dir) => {
                    let providers = capability::print_policy_grants(&layout, &config_dir);
                    capability::print_alias_routes(&layout, &providers);
                }
                None => emit(
                    "=== policy grants against authority routes\nnot compared: the serving generation's trust config directory is unknown, as the service env section says".into(),
                ),
            }
            reachability::print_reachability(&layout).await;
            reachability::print_boot_attempt(&layout);
        }
        Err(cause) => emit(format!("=== installation\n{cause}")),
    }
    let Some(lines) = COLLECTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    else {
        return;
    };
    // A line `=== title` opens a section; every other line belongs to the
    // section open at that point, exactly as the terminal shows it.
    let mut sections: Vec<serde_json::Value> = Vec::new();
    for line in lines
        .iter()
        .flat_map(|line| line.split('\n'))
        .filter(|line| !line.is_empty())
    {
        if let Some(title) = line.strip_prefix("=== ") {
            sections.push(serde_json::json!({ "title": title, "lines": [] }));
        } else if let Some(open) = sections
            .last_mut()
            .and_then(|section| section["lines"].as_array_mut())
        {
            open.push(Value::String(line.to_string()));
        }
    }
    let document = serde_json::json!({ "sections": sections });
    println!("{document:#}");
}
