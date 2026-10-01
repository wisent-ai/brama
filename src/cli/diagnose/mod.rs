//! `brama diagnose`: answer, on one host, why this gateway is or is not
//! serving.
//!
//! Every failure a host has had reads the same from outside — `/health`
//! answers and no route works, or nothing listens — while the cause was a
//! different file each time. So this opens all of them, in the order the
//! launcher does, and prints what each says beside what it has to agree with:
//!
//! 1. the units that start Brama, and the release state that actually serves;
//! 2. every installed generation: completeness, router verbs, and whether the
//!    trust registry describes that exact installation;
//! 3. the service env beside the supervisor-owned runtime coordinates;
//! 4. the policy's provider grants against the authority-owned routes table;
//! 5. every alias route against the providers policy and routes agree on;
//! 6. where the gateway is reachable, and by which scheme;
//! 7. the current boot attempt and the ends of both log streams.
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
}

impl Layout {
    fn read() -> Self {
        let home = super::workload::home();
        let settings = service_settings(&home).unwrap_or_default();
        let release_state = std::fs::read(home.join(".stado/release-state/brama.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(Value::Null);
        Layout {
            services: home.join(".stado/services/brama"),
            log: home
                .join(".stado/logs")
                .join(format!("{SERVICE_LABEL}.log")),
            home,
            settings,
            release_state,
        }
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

    fn release_root(&self, name: &str) -> Option<PathBuf> {
        let dir = self.release_record(name)?.get("release_dir")?.as_str()?;
        std::fs::canonicalize(dir).ok()
    }

    fn config_dir_for(&self, generation: &Path) -> PathBuf {
        let name = generation
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.home
            .join(".config/brama")
            .join(format!("trust-{name}"))
    }
}

/// Seconds since the epoch as `YYYY-MM-DDTHH:MM:SSZ`.
pub(super) fn moment(path: &Path) -> String {
    std::fs::symlink_metadata(path)
        .and_then(|meta| meta.modified())
        .map(|time| {
            chrono::DateTime::<chrono::Utc>::from(time)
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
        })
        .unwrap_or_else(|_| "unknown".into())
}

pub(super) fn read_json(path: &Path) -> Option<Value> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

pub(crate) async fn run(json: bool) {
    if json {
        *COLLECTED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Vec::new());
    }
    let layout = Layout::read();
    let resolved = installation::print_units(&layout);
    installation::print_generations(&layout, resolved.as_deref());
    let config_dir = installation::print_service_env(&layout, resolved.as_deref());
    let providers = capability::print_policy_grants(&layout, &config_dir);
    capability::print_alias_routes(&layout, &providers);
    reachability::print_reachability(&layout).await;
    reachability::print_boot_attempt(&layout);
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
    println!(
        "{}",
        serde_json::to_string_pretty(&document).unwrap_or_else(|_| "{}".into())
    );
}
