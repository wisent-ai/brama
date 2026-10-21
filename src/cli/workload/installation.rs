//! What `register` and `deregister` read before they act: the home
//! directory, the service settings, and the installation they name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The home directory, from the one lookup every Brama path uses: HOME, else
/// the user database's home, else the cause.
pub(crate) use brama::journal::home;

/// `~/.config/brama/service.env` as `NAME=value` settings, quotes removed.
pub(crate) fn service_settings(home: &Path) -> Result<BTreeMap<String, String>, String> {
    let path = home.join(".config/brama/service.env");
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(name, _)| !name.trim_start().starts_with('#'))
        .map(|(name, value)| {
            (
                name.trim().to_string(),
                value.trim().trim_matches(['\'', '"']).to_string(),
            )
        })
        .collect())
}

/// What `register` and `deregister` both act through: the entitlements
/// router, the vault, the workload registry, and the service settings the
/// router runs with.
pub(super) struct Installation {
    pub(super) home: PathBuf,
    pub(super) settings: BTreeMap<String, String>,
    pub(super) router: String,
    pub(super) vault: String,
    pub(super) registry_path: PathBuf,
    pub(super) workload: Value,
}

/// The running installation's file root, when one is installed: `current`
/// resolved, then its generation root. An installation that is not there is
/// no answer; one that cannot be resolved is refused.
fn running_root(home: &Path) -> Result<Option<PathBuf>, String> {
    let current = home.join(".stado/services/brama/current");
    let running = match std::fs::canonicalize(&current) {
        Ok(running) => running,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "cannot resolve the running installation {}: {error}",
                current.display()
            ))
        }
    };
    crate::cli::generation::generation_root(&running).map(Some)
}

pub(super) fn installation() -> Result<Installation, String> {
    let home = home()?;
    let settings = service_settings(&home)?;
    // Set and not UTF-8 is a broken value, not an absent one: passing over it
    // would register from a path the launcher did not name.
    let env = brama::environment::declared_setting;
    let setting = |name: &str| {
        settings
            .get(name)
            .cloned()
            .filter(|value| !value.is_empty())
    };
    // The launcher passes these directly, and then they are authoritative: it
    // has just provisioned the directory whose key must be registered. Outside
    // the launcher, `current` is authoritative for executable and trust
    // material; service.env can keep paths from an older digest.
    let root = running_root(&home)?;
    let current_router = root
        .as_ref()
        .map(|root| root.join("bin/skarbiec-entitlements-router"))
        .filter(|path| path.is_file());
    let current_config = root
        .as_ref()
        .map(|root| root.join("etc/brama-skarbiec"))
        .filter(|path| path.is_dir());
    let router = env("ENTITLEMENTS_ROUTER_BIN")?
        .or_else(|| current_router.map(|path| path.display().to_string()))
        .or_else(|| setting("ENTITLEMENTS_ROUTER_BIN"));
    let vault = env("SKARBIEC_VAULT_FILE")?.or_else(|| setting("SKARBIEC_VAULT_FILE"));
    let config_dir = env("BRAMA_SKARBIEC_CONFIG_DIR")?
        .or_else(|| current_config.map(|path| path.display().to_string()))
        .or_else(|| setting("BRAMA_SKARBIEC_CONFIG_DIR"));
    let (Some(router), Some(vault), Some(config_dir)) = (router, vault, config_dir) else {
        return Err("ENTITLEMENTS_ROUTER_BIN, SKARBIEC_VAULT_FILE and BRAMA_SKARBIEC_CONFIG_DIR must be named by the running installation, environment, or service env".into());
    };
    let registry_path = PathBuf::from(config_dir).join("registry.json");
    let registry: Value = match std::fs::read(&registry_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "workload registry {} is not JSON: {error}",
                registry_path.display()
            )
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "no workload registry at {}",
                registry_path.display()
            ))
        }
        Err(error) => {
            return Err(format!(
                "cannot read workload registry {}: {error}",
                registry_path.display()
            ))
        }
    };
    let workload = registry
        .get("workloads")
        .and_then(Value::as_object)
        .and_then(|workloads| workloads.values().next())
        .cloned()
        .ok_or_else(|| {
            format!(
                "workload registry {} declares no workload",
                registry_path.display()
            )
        })?;
    Ok(Installation {
        home,
        settings,
        router,
        vault,
        registry_path,
        workload,
    })
}

/// The agents the registry's workload names. An entry that is not text is
/// refused, so no agent is silently left ungranted on register or unrevoked
/// on deregister.
pub(super) fn agent_ids(workload: &Value) -> Result<Vec<&str>, String> {
    workload
        .get("agent_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|agent| {
            agent.as_str().ok_or_else(|| {
                format!("the workload registry lists an agent id that is not text: {agent}")
            })
        })
        .collect()
}

/// The router's own words for a refusal: stderr, or stdout when stderr is empty.
pub(super) fn refusal_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = if stderr.trim().is_empty() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        stderr.trim().to_string()
    };
    detail.replace('\n', " ")
}
