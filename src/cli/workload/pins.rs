//! The broker pins the process allowed to redeem a capability: uid, gid,
//! resolved executable path and SHA-256. A registry provisioned for another
//! installation names someone else. `brama workload check` and
//! `brama diagnose` both compare through here.

use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Every field the registry's workload pins differently from `binary` run by
/// this user, as `name pinned=… actual=…`; empty when it describes it.
pub(crate) fn mismatches(registry: &Path, binary: &Path) -> Result<Vec<String>, String> {
    let bytes = std::fs::read(registry).map_err(|error| {
        format!(
            "the workload registry {} cannot be read: {error}",
            registry.display()
        )
    })?;
    let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "the workload registry {} is not JSON: {error}",
            registry.display()
        )
    })?;
    let workloads = document
        .get("workloads")
        .and_then(Value::as_object)
        .filter(|workloads| !workloads.is_empty())
        .ok_or_else(|| {
            format!(
                "workload registry {} declares no workload",
                registry.display()
            )
        })?;
    let bytes = std::fs::read(binary).map_err(|error| format!("{}: {error}", binary.display()))?;
    // SAFETY: getuid and getgid cannot fail and touch no memory.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let path =
        std::fs::canonicalize(binary).map_err(|error| format!("{}: {error}", binary.display()))?;
    let shown = path.display().to_string();
    // The workload this binary is: the one pinning its path. A registry of
    // one workload is compared against it even when it names another path,
    // which is the mismatch this check exists to report; several workloads
    // none of which names this binary cannot be told apart.
    let mut naming = workloads.values().filter(|workload| {
        workload.get("executable_path").and_then(Value::as_str) == Some(shown.as_str())
    });
    let mut every = workloads.values();
    let workload = match (naming.next(), naming.next(), every.next(), every.next()) {
        (Some(one), None, _, _) => one,
        (None, _, Some(only), None) => only,
        (Some(_first), Some(_second), _, _) => {
            return Err(format!(
                "workload registry {} pins {shown} for more than one workload",
                registry.display()
            ))
        }
        _ => {
            return Err(format!(
                "workload registry {} declares workloads {} and none pins {shown}",
                registry.display(),
                workloads.keys().cloned().collect::<Vec<_>>().join(", ")
            ))
        }
    };
    let expected = [
        ("uid", uid.to_string()),
        ("gid", gid.to_string()),
        ("executable_path", path.display().to_string()),
        ("executable_sha256", hex::encode(Sha256::digest(&bytes))),
    ];
    Ok(expected
        .into_iter()
        .filter_map(|(name, actual)| match workload.get(name) {
            Some(Value::String(pinned)) => {
                (*pinned != actual).then(|| format!("{name} pinned={pinned} actual={actual}"))
            }
            Some(Value::Null) | None => Some(format!("{name} is not pinned, actual={actual}")),
            Some(other) => Some(format!(
                "{name} is pinned as {other}, which is not text, actual={actual}"
            )),
        })
        .collect())
}
