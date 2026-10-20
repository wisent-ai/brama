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
    let document: Value = std::fs::read(registry)
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|error| error.to_string()))
        .map_err(|error| format!("workload registry is unreadable: {error}"))?;
    let workload = document
        .get("workloads")
        .and_then(Value::as_object)
        .and_then(|workloads| workloads.values().next())
        .cloned()
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
