//! Where the catalog document comes from, and what address Brama will fetch.
//!
//! The document is models.dev's published API unless `BRAMA_MODEL_CATALOG_URL`
//! names a mirror, or `BRAMA_MODEL_CATALOG_PATH` a file. A configured address
//! is checked before it is used: HTTPS, or plain HTTP to a loopback IP address
//! for a local mirror, and never credentials embedded in the URL. The on-disk
//! copy (`BRAMA_MODEL_CATALOG_CACHE`, else Brama's state directory) is written
//! next to its own path and renamed into place, so a reader never sees half a
//! document.

use std::path::PathBuf;
use std::sync::LazyLock;

/// Where models.dev publishes its catalog: https://models.dev ("api.json").
const MODELS_DEV_CATALOG_URL: &str = "https://models.dev/api.json";
/// The catalog copy's file name inside Brama's state directory
/// (`journal::state_dir`); `/tmp` is shared by every program on the host and
/// is not where a product keeps its state.
const CACHE_FILE: &str = "models-dev-cache.json";

fn catalog_url() -> Result<reqwest::Url, String> {
    let raw = match crate::environment::declared_text("BRAMA_MODEL_CATALOG_URL")? {
        Some(raw) => raw,
        None => MODELS_DEV_CATALOG_URL.to_string(),
    };
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|error| format!("invalid model catalog URL {raw:?}: {error}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("model catalog URL must not contain user info".into());
    }
    let loopback = match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(_)) => false,
        None => return Err(format!("model catalog URL {raw:?} names no host")),
    };
    let secure = url.scheme() == "https";
    let local = url.scheme() == "http" && loopback;
    if !secure && !local {
        return Err(format!(
            "model catalog URL {raw:?} must use HTTPS, or plain HTTP to a loopback IP address"
        ));
    }
    Ok(url)
}

pub(super) async fn read_live_catalog() -> Result<String, String> {
    if let Some(path) = crate::environment::declared_text("BRAMA_MODEL_CATALOG_PATH")? {
        return tokio::fs::read_to_string(&path)
            .await
            .map_err(|error| format!("cannot read model catalog {path}: {error}"));
    }
    let url = catalog_url()?;
    // One client for the refresh, not one per refresh: each `Client` carries its
    // own connection pool, and a new one every cycle leaks the sockets of the
    // last one until the process runs out of descriptors.
    static CATALOG_CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                format!(
                    "the model catalog's HTTP client cannot be built: {}",
                    crate::core::failure::error_chain(&error)
                )
            })
    });
    let client = CATALOG_CLIENT.clone()?;
    let response = stado_wait::request(client.get(url.clone()))
        .await
        .map_err(|error| format!("model catalog {url}: {error}"))?
        .error_for_status()
        .map_err(|error| format!("model catalog {url}: {error}"))?;
    response
        .text()
        .await
        .map_err(|error| format!("model catalog {url}: {error}"))
}

fn cache_path() -> Result<PathBuf, String> {
    Ok(
        match crate::environment::declared_text("BRAMA_MODEL_CATALOG_CACHE")? {
            Some(path) => PathBuf::from(path),
            None => crate::journal::state_dir()?.join(CACHE_FILE),
        },
    )
}

/// The last fetched copy and when it was written (RFC 3339, from the file's
/// modification time).
pub(super) async fn read_cache() -> Result<(String, String), String> {
    let path = cache_path()?;
    let raw = tokio::fs::read_to_string(&path)
        .await
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let written = tokio::fs::metadata(&path)
        .await
        .and_then(|metadata| metadata.modified())
        .map_err(|error| format!("cannot read when {} was written: {error}", path.display()))?;
    let written_at = chrono::DateTime::<chrono::Utc>::from(written).to_rfc3339();
    Ok((raw, written_at))
}

pub(super) async fn write_cache(raw: &str) -> Result<(), String> {
    let path = cache_path()?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    tokio::fs::write(&temporary, raw)
        .await
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    tokio::fs::rename(&temporary, &path).await.map_err(|error| {
        format!(
            "cannot move {} to {}: {error}",
            temporary.display(),
            path.display()
        )
    })
}
