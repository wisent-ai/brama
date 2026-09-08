//! Where the catalog document comes from, and what address Brama will fetch.
//!
//! The address is operator-configurable, so it is checked before it is used:
//! HTTPS, or explicit loopback HTTP for a local mirror, and never credentials
//! embedded in the URL. The on-disk copy is written next to its own path and
//! renamed into place, so a reader never sees half a document.

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

const DEFAULT_CATALOG_URL: &str = "https://models.dev/api.json";
const DEFAULT_CACHE_PATH: &str = "/tmp/brama-models-dev-cache.json";

fn catalog_url() -> Result<reqwest::Url, String> {
    let raw = std::env::var("BRAMA_MODEL_CATALOG_URL")
        .unwrap_or_else(|_| DEFAULT_CATALOG_URL.to_string());
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|error| format!("invalid model catalog URL: {error}"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("model catalog URL must not contain user info".into());
    }
    let host = url
        .host_str()
        .ok_or_else(|| "model catalog URL must contain a host".to_string())?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("model catalog URL must use HTTPS or explicit loopback HTTP".into());
    }
    Ok(url)
}

pub(super) async fn read_live_catalog() -> Result<String, String> {
    if let Ok(path) = std::env::var("BRAMA_MODEL_CATALOG_PATH") {
        return tokio::fs::read_to_string(path)
            .await
            .map_err(|error| error.to_string());
    }
    let url = catalog_url()?;
    // One client for the refresh, not one per refresh: each `Client` carries its
    // own connection pool, and a new one every cycle leaks the sockets of the
    // last one until the process runs out of descriptors.
    static CATALOG_CLIENT: LazyLock<Result<reqwest::Client, String>> = LazyLock::new(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())
    });
    let client = CATALOG_CLIENT.clone()?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    response.text().await.map_err(|error| error.to_string())
}

fn cache_path() -> PathBuf {
    std::env::var("BRAMA_MODEL_CATALOG_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_CACHE_PATH))
}

pub(super) async fn read_cache() -> Result<String, String> {
    tokio::fs::read_to_string(cache_path())
        .await
        .map_err(|error| error.to_string())
}

pub(super) async fn write_cache(raw: &str) -> Result<(), String> {
    let path = cache_path();
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    tokio::fs::write(&temporary, raw)
        .await
        .map_err(|error| error.to_string())?;
    tokio::fs::rename(&temporary, &path)
        .await
        .map_err(|error| error.to_string())
}
