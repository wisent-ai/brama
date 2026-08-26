use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ROUTES_FILE_ENV: &str = "BRAMA_INFERENCE_ROUTES_FILE";

#[derive(Debug, Deserialize, Serialize)]
struct Endpoint {
    host: String,
    port: u16,
}

#[derive(Debug, Deserialize, Serialize)]
struct Adapter {
    name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Deployment {
    name: String,
    #[serde(default)]
    adapters: Vec<Adapter>,
    endpoint: Endpoint,
}

#[derive(Debug, Deserialize, Serialize)]
struct Registry {
    #[serde(default)]
    deployments: Vec<Deployment>,
    #[serde(default)]
    routes: HashMap<String, String>,
    #[serde(default)]
    fallbacks: HashMap<String, Vec<String>>,
}

static ROUTE_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn read_body(path: &Path) -> Result<String, String> {
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

fn read(path: &Path) -> Result<Registry, String> {
    let body = read_body(path)?;
    serde_json::from_str(&body).map_err(|error| format!("invalid inference routes: {error}"))
}

pub fn configured_path() -> Option<PathBuf> {
    std::env::var_os(ROUTES_FILE_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
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
    for (alias, destinations) in &registry.fallbacks {
        if !registry.routes.contains_key(alias) {
            return Err(format!(
                "inference fallback route '{alias}' has no primary destination"
            ));
        }
        for destination in destinations {
            resolved_destination(&registry, destination)?;
        }
    }
    Ok(value)
}

pub fn update_route(
    path: &Path,
    alias: &str,
    primary: &str,
    fallbacks: &[String],
) -> Result<Value, String> {
    let _guard = ROUTE_WRITE_LOCK
        .lock()
        .map_err(|_| "inference route write lock is poisoned".to_string())?;
    let mut value = snapshot(path)?;
    let document = value
        .as_object_mut()
        .ok_or_else(|| "inference routes must be a JSON object".to_string())?;
    let routes = document
        .entry("routes")
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object_mut()
        .ok_or_else(|| "inference routes.routes must be an object".to_string())?;
    routes.insert(alias.to_string(), Value::String(primary.to_string()));
    let fallback_map = document
        .entry("fallbacks")
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object_mut()
        .ok_or_else(|| "inference routes.fallbacks must be an object".to_string())?;
    if fallbacks.is_empty() {
        fallback_map.remove(alias);
    } else {
        fallback_map.insert(
            alias.to_string(),
            Value::Array(fallbacks.iter().cloned().map(Value::String).collect()),
        );
    }

    let bytes = serde_json::to_vec_pretty(&value)
        .map_err(|error| format!("cannot encode inference routes: {error}"))?;
    let parent = path
        .parent()
        .ok_or_else(|| "inference routes path has no parent".to_string())?;
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
        Ok(value)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    result
}

pub fn delete_route(path: &Path, alias: &str) -> Result<Value, String> {
    let _guard = ROUTE_WRITE_LOCK
        .lock()
        .map_err(|_| "inference route write lock is poisoned".to_string())?;
    let mut value = snapshot(path)?;
    let document = value
        .as_object_mut()
        .ok_or_else(|| "inference routes must be a JSON object".to_string())?;
    let routes = document
        .get_mut("routes")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "inference routes.routes must be an object".to_string())?;
    if routes.remove(alias).is_none() {
        return Err("route alias not found".to_string());
    }
    if let Some(fallbacks) = document.get_mut("fallbacks").and_then(Value::as_object_mut) {
        fallbacks.remove(alias);
    }
    write_registry(path, &value)?;
    Ok(value)
}

fn write_registry(path: &Path, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("cannot encode inference routes: {error}"))?;
    let parent = path
        .parent()
        .ok_or_else(|| "inference routes path has no parent".to_string())?;
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
fn safe_inference_host(value: &str) -> bool {
    let Ok(address) = value.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    if address.is_loopback() {
        return true;
    }
    let octets = address.octets();
    let first = "100".parse::<u8>().expect("static Tailscale prefix");
    let lower = "64".parse::<u8>().expect("static Tailscale range");
    let upper = "128".parse::<u8>().expect("static Tailscale range");
    octets[usize::MIN] == first && (lower..upper).contains(&octets[usize::from(true)])
}

fn deployment_for_model<'a>(registry: &'a Registry, model: &str) -> Result<&'a Deployment, String> {
    let mut matches = registry.deployments.iter().filter(|deployment| {
        deployment.name == model
            || deployment
                .adapters
                .iter()
                .any(|adapter| adapter.name == model)
    });
    let deployment = matches
        .next()
        .ok_or_else(|| format!("unknown local inference model '{model}'"))?;
    if matches.next().is_some() {
        return Err(format!("ambiguous local inference model '{model}'"));
    }
    Ok(deployment)
}

/// A destination naming `best` is delegation, not a local deployment: the
/// operator is saying "whatever the subscription route picks", so it passes
/// through untouched and subscription dispatch resolves it per caller identity.
///
/// Without this, `best` carries no slash, falls into the deployment lookup
/// below and is rejected as an unknown local model. That forced every route to
/// name one fixed provider and model, and left no way to point an alias at the
/// subscription that pays.
fn resolved_destination(registry: &Registry, destination: &str) -> Result<String, String> {
    if destination == crate::core::server::BEST_ALIAS {
        return Ok(destination.to_string());
    }
    if destination.contains('/') {
        return Ok(destination.to_string());
    }
    let deployment = deployment_for_model(registry, destination)?;
    if !safe_inference_host(&deployment.endpoint.host) || deployment.endpoint.port == u16::MIN {
        return Err(format!(
            "inference deployment '{destination}' has no safe local or Tailscale endpoint"
        ));
    }
    Ok(format!("local-openai/{destination}"))
}

pub fn resolved(path: &Path) -> Result<HashMap<String, String>, String> {
    let registry = read(path)?;
    let mut routes = HashMap::new();
    for (alias, destination) in &registry.routes {
        routes.insert(alias.clone(), resolved_destination(&registry, destination)?);
    }
    Ok(routes)
}

pub fn resolved_fallbacks(path: &Path) -> Result<HashMap<String, Vec<String>>, String> {
    let registry = read(path)?;
    let mut fallbacks = HashMap::new();
    for (alias, destinations) in &registry.fallbacks {
        if !registry.routes.contains_key(alias) {
            return Err(format!(
                "inference fallback route '{alias}' has no primary destination"
            ));
        }
        let primary = registry
            .routes
            .get(alias)
            .ok_or_else(|| format!("inference route '{alias}' disappeared"))?;
        let mut seen = std::collections::HashSet::from([primary.as_str()]);
        let mut resolved = Vec::with_capacity(destinations.len());
        for destination in destinations {
            if !seen.insert(destination.as_str()) {
                return Err(format!(
                    "inference route '{alias}' repeats destination '{destination}'"
                ));
            }
            resolved.push(resolved_destination(&registry, destination)?);
        }
        fallbacks.insert(alias.clone(), resolved);
    }
    Ok(fallbacks)
}

pub fn base_url(path: &Path, model_name: &str) -> Result<String, String> {
    let registry = read(path)?;
    let deployment = deployment_for_model(&registry, model_name)?;
    if !safe_inference_host(&deployment.endpoint.host) || deployment.endpoint.port == u16::MIN {
        return Err(format!(
            "inference model '{model_name}' has no safe local or Tailscale endpoint"
        ));
    }
    Ok(format!(
        "http://{}:{}",
        deployment.endpoint.host, deployment.endpoint.port
    ))
}

pub fn validate(path: &Path) -> Result<(), String> {
    resolved(path)?;
    resolved_fallbacks(path).map(|_| ())
}

pub fn resolve(path: &Path, alias: &str) -> Result<Option<String>, String> {
    Ok(resolved(path)?.remove(alias))
}

pub fn route_chain(path: &Path, alias: &str) -> Result<Option<Vec<String>>, String> {
    let registry = read(path)?;
    let Some(primary) = registry.routes.get(alias) else {
        return Ok(None);
    };
    let fallbacks = registry
        .fallbacks
        .get(alias)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut seen = std::collections::HashSet::from([primary.as_str()]);
    let mut chain = Vec::with_capacity(fallbacks.len().saturating_add(usize::from(true)));
    chain.push(resolved_destination(&registry, primary)?);
    for fallback in fallbacks {
        if !seen.insert(fallback.as_str()) {
            return Err(format!(
                "inference route '{alias}' repeats destination '{fallback}'"
            ));
        }
        chain.push(resolved_destination(&registry, fallback)?);
    }
    Ok(Some(chain))
}
