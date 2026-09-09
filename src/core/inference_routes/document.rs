//! The route-registry document: the typed shape Brama accepts, the refusals a
//! file has to survive before it is read at all, and the one way a new version
//! reaches disk.
//!
//! Every writer in this family goes through [`write_registry`], so the staging
//! file, its owner-only mode, the sync, the re-validation of the staged bytes
//! and the rename are described once and cannot drift apart between the route
//! editor, the migration and the importer.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::resolution::{resolved_destination, safe_inference_host, validate};

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Endpoint {
    pub(super) host: String,
    pub(super) port: u16,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Adapter {
    pub(super) name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Deployment {
    pub(super) name: String,
    #[serde(default)]
    pub(super) adapters: Vec<Adapter>,
    pub(super) endpoint: Endpoint,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Registry {
    #[serde(default)]
    pub(super) deployments: Vec<Deployment>,
    #[serde(default)]
    pub(super) routes: HashMap<String, String>,
}

/// The only registry schema version Brama reads; a document that never wrote
/// the member down is such a document, so writers state it explicitly.
pub(super) const SCHEMA_VERSION: u32 = 1;

pub(super) static ROUTE_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub(super) fn read_body(path: &Path) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot read inference routes metadata: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("inference routes must be a regular non-symlink file".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        const NON_OWNER_MASK: u32 = 0o077;
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err("inference routes must be owned by the Brama user".to_string());
        }
        if metadata.permissions().mode() & NON_OWNER_MASK != u32::MIN {
            return Err("inference routes must not be accessible by group or other".to_string());
        }
    }
    std::fs::read_to_string(path).map_err(|error| format!("cannot read inference routes: {error}"))
}

pub(super) fn read(path: &Path) -> Result<Registry, String> {
    let body = read_body(path)?;
    serde_json::from_str(&body).map_err(|error| format!("invalid inference routes: {error}"))
}

pub fn snapshot(path: &Path) -> Result<Value, String> {
    let body = read_body(path)?;
    let value: Value = serde_json::from_str(&body)
        .map_err(|error| format!("invalid inference routes: {error}"))?;
    let registry: Registry = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid inference routes: {error}"))?;
    for destination in registry.routes.values() {
        resolved_destination(&registry, destination)?;
    }
    Ok(value)
}

/// Validate one complete route-registry document without touching disk.
///
/// Imports use this before opening the destination, so malformed routes and
/// unsafe deployment endpoints cannot leave a partial registry behind.
pub fn validate_document(value: &Value) -> Result<(), String> {
    let registry: Registry = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid inference routes: {error}"))?;
    let mut deployment_names = HashSet::new();
    for deployment in &registry.deployments {
        if deployment.name.is_empty()
            || !deployment_names.insert(deployment.name.as_str())
            || !safe_inference_host(&deployment.endpoint.host)
            || deployment.endpoint.port == u16::MIN
        {
            return Err(format!(
                "inference deployment '{}' is duplicated or has no safe local or Tailscale endpoint",
                deployment.name
            ));
        }
    }
    for destination in registry.routes.values() {
        resolved_destination(&registry, destination)?;
    }
    Ok(())
}

pub(super) fn route_file_exists(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err("inference routes must be a regular non-symlink file".to_string())
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot read inference routes metadata: {error}")),
    }
}

pub(super) fn ensure_parent(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create inference routes directory: {error}"))
}

pub(super) fn write_registry(path: &Path, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("cannot encode inference routes: {error}"))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "inference routes path has no safe file name".to_string())?;
    let staging = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let result = (|| {
        #[cfg(unix)]
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&staging)
            .map_err(|error| format!("cannot create route staging file: {error}"))?;
        file.write_all(&bytes)
            .map_err(|error| format!("cannot write route staging file: {error}"))?;
        file.write_all(b"\n")
            .map_err(|error| format!("cannot finish route staging file: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("cannot sync route staging file: {error}"))?;
        validate(&staging)?;
        std::fs::rename(&staging, path)
            .map_err(|error| format!("cannot commit inference routes: {error}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    result
}
