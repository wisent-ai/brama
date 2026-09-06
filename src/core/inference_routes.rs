use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ROUTES_FILE_ENV: &str = "BRAMA_INFERENCE_ROUTES_FILE";

#[derive(Debug, Clone)]
pub struct RouteImport {
    pub alias: String,
    pub primary: String,
    pub fallbacks: Vec<String>,
    pub deployments: Vec<Value>,
    pub expected_primary: Option<String>,
    pub expected_fallbacks: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteImportDisposition {
    Imported,
    Unchanged,
    Conflicting,
}

#[derive(Debug, Clone)]
pub struct RouteImportResult {
    pub alias: String,
    pub disposition: RouteImportDisposition,
    pub detail: String,
}

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

/// Validate one complete route-registry document without touching disk.
///
/// Imports use this before opening the destination, so malformed routes,
/// duplicate fallbacks, and unsafe deployment endpoints cannot leave a partial
/// registry behind.
pub fn validate_document(value: &Value) -> Result<(), String> {
    let registry: Registry = serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid inference routes: {error}"))?;
    let mut deployment_names = std::collections::HashSet::new();
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
    for (alias, destinations) in &registry.fallbacks {
        let primary = registry.routes.get(alias).ok_or_else(|| {
            format!("inference fallback route '{alias}' has no primary destination")
        })?;
        let mut seen = std::collections::HashSet::from([primary.as_str()]);
        for destination in destinations {
            if !seen.insert(destination.as_str()) {
                return Err(format!(
                    "inference route '{alias}' repeats destination '{destination}'"
                ));
            }
            resolved_destination(&registry, destination)?;
        }
    }
    Ok(())
}

/// Merge selected aliases from a previously validated import in one atomic
/// destination rewrite. Existing aliases and deployments win by default.
///
/// A deployment-name conflict is never replaced: doing so could silently
/// redirect an existing alias that was not part of the user's selection.
pub fn import_routes(
    path: &Path,
    imports: &[RouteImport],
    replace_alias_conflicts: bool,
) -> Result<(Value, Vec<RouteImportResult>), String> {
    let _guard = ROUTE_WRITE_LOCK
        .lock()
        .map_err(|_| "inference route write lock is poisoned".to_string())?;
    let mut value = if route_file_exists(path)? {
        snapshot(path)?
    } else {
        serde_json::json!({
            "schema_version": 1,
            "deployments": [],
            "routes": {},
            "fallbacks": {},
        })
    };
    validate_document(&value)?;
    let document = value
        .as_object_mut()
        .ok_or_else(|| "inference routes must be a JSON object".to_string())?;
    let existing_routes = document
        .entry("routes")
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object()
        .ok_or_else(|| "inference routes.routes must be an object".to_string())?
        .clone();
    let existing_fallbacks = document
        .entry("fallbacks")
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object()
        .ok_or_else(|| "inference routes.fallbacks must be an object".to_string())?
        .clone();
    let existing_deployments = document
        .entry("deployments")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array()
        .ok_or_else(|| "inference routes.deployments must be an array".to_string())?
        .clone();

    let mut deployment_by_name = HashMap::new();
    for deployment in existing_deployments {
        let name = deployment
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "inference route deployment has no name".to_string())?;
        deployment_by_name.insert(name.to_string(), deployment);
    }

    let mut results = Vec::with_capacity(imports.len());
    let mut accepted = Vec::new();
    for route in imports {
        let imported_primary = Value::String(route.primary.clone());
        let imported_fallbacks =
            Value::Array(route.fallbacks.iter().cloned().map(Value::String).collect());
        let current_primary = existing_routes.get(&route.alias);
        let current_fallbacks = existing_fallbacks
            .get(&route.alias)
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        let expected_primary = route
            .expected_primary
            .as_ref()
            .map(|primary| Value::String(primary.clone()));
        let expected_fallbacks = Value::Array(
            route
                .expected_fallbacks
                .iter()
                .cloned()
                .map(Value::String)
                .collect(),
        );
        if current_primary != expected_primary.as_ref() || current_fallbacks != expected_fallbacks {
            results.push(RouteImportResult {
                alias: route.alias.clone(),
                disposition: RouteImportDisposition::Conflicting,
                detail: "the destination changed after the adoption review".to_string(),
            });
            continue;
        }
        if current_primary == Some(&imported_primary) && current_fallbacks == imported_fallbacks {
            results.push(RouteImportResult {
                alias: route.alias.clone(),
                disposition: RouteImportDisposition::Unchanged,
                detail: "the destination already has this route chain".to_string(),
            });
            continue;
        }
        if current_primary.is_some() && !replace_alias_conflicts {
            results.push(RouteImportResult {
                alias: route.alias.clone(),
                disposition: RouteImportDisposition::Conflicting,
                detail: "the destination keeps its existing alias".to_string(),
            });
            continue;
        }
        let deployment_conflict = route.deployments.iter().find_map(|deployment| {
            let name = deployment.get("name").and_then(Value::as_str)?;
            deployment_by_name
                .get(name)
                .filter(|existing| *existing != deployment)
                .map(|_| name.to_string())
        });
        if let Some(name) = deployment_conflict {
            results.push(RouteImportResult {
                alias: route.alias.clone(),
                disposition: RouteImportDisposition::Conflicting,
                detail: format!(
                    "deployment '{name}' already exists with a different endpoint or adapter"
                ),
            });
            continue;
        }
        for deployment in &route.deployments {
            let name = deployment
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| "inference route deployment has no name".to_string())?;
            deployment_by_name
                .entry(name.to_string())
                .or_insert_with(|| deployment.clone());
        }
        accepted.push(route);
    }

    if !accepted.is_empty() {
        for route in accepted {
            document
                .get_mut("routes")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| "inference routes.routes must be an object".to_string())?
                .insert(route.alias.clone(), Value::String(route.primary.clone()));
            let fallback_map = document
                .get_mut("fallbacks")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| "inference routes.fallbacks must be an object".to_string())?;
            if route.fallbacks.is_empty() {
                fallback_map.remove(&route.alias);
            } else {
                fallback_map.insert(
                    route.alias.clone(),
                    Value::Array(route.fallbacks.iter().cloned().map(Value::String).collect()),
                );
            }
            results.push(RouteImportResult {
                alias: route.alias.clone(),
                disposition: RouteImportDisposition::Imported,
                detail: "the selected route chain was persisted".to_string(),
            });
        }
        let mut deployments = deployment_by_name.into_values().collect::<Vec<_>>();
        deployments.sort_by(|left, right| {
            left.get("name")
                .and_then(Value::as_str)
                .cmp(&right.get("name").and_then(Value::as_str))
        });
        document.insert("deployments".to_string(), Value::Array(deployments));
        validate_document(&value)?;
        ensure_parent(path)?;
        write_registry(path, &value)?;
    }
    results.sort_by(|left, right| left.alias.cmp(&right.alias));
    Ok((value, results))
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

fn route_file_exists(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err("inference routes must be a regular non-symlink file".to_string())
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot read inference routes metadata: {error}")),
    }
}

fn ensure_parent(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create inference routes directory: {error}"))
}

fn write_registry(path: &Path, value: &Value) -> Result<(), String> {
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
