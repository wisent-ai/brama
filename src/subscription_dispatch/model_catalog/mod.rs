//! Independent Wisent model catalog.
//!
//! models.dev supplies public provider/model metadata only. Skarbiec remains the
//! credential authority and Weles remains the account/subscription authority.
//!
//! This file owns how fresh that metadata has to be: one snapshot in memory,
//! one loader at a time, and a TTL the operator sets. Where the document comes
//! from is `source`, what a provider in it means is `provider`, and turning it
//! into routable models is `parse`.

mod parse;
mod provider;
mod source;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex, RwLock};

use crate::providers::adapter::RegistryModel;

use parse::parse_catalog;
use source::{read_cache, read_live_catalog, write_cache};

pub use provider::{CatalogAuth, CatalogProtocol, CatalogProvider};

/// The age at which a loaded catalog is fetched again, in seconds. Unset, a
/// loaded catalog is kept for the life of the process: no age is assumed.
const TTL_ENV: &str = "BRAMA_MODEL_CATALOG_TTL_SECONDS";

#[derive(Clone, Debug)]
pub struct CatalogSnapshot {
    pub providers: HashMap<String, CatalogProvider>,
    pub models: Vec<RegistryModel>,
    pub revision: String,
}

/// Where the catalog in memory came from.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CatalogSource {
    /// Read on this load from models.dev, its configured mirror or file.
    Live,
    /// The copy last written to disk, because the live read failed; when the
    /// copy was written and why the live read failed.
    LastFetchedCopy {
        written_at: String,
        live_error: String,
    },
}

struct CachedSnapshot {
    loaded_at: Instant,
    snapshot: Arc<CatalogSnapshot>,
    source: CatalogSource,
}

static MEMORY_CACHE: LazyLock<RwLock<Option<CachedSnapshot>>> = LazyLock::new(|| RwLock::new(None));
static LOAD_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// The catalog as of no longer ago than the configured TTL.
///
/// The load lock is why a cold start under concurrent traffic fetches the
/// document once instead of once per caller, and every waiter re-reads the
/// cache after taking it, so the fetch that already happened is the one they
/// all get.
pub async fn snapshot() -> Result<Arc<CatalogSnapshot>, String> {
    let ttl = crate::types::declared_age(TTL_ENV)?;
    if let Some(fresh) = cached_within(ttl).await {
        return Ok(fresh);
    }
    let _load_guard = LOAD_LOCK.lock().await;
    if let Some(fresh) = cached_within(ttl).await {
        return Ok(fresh);
    }

    let (raw, source) = match read_live_catalog().await {
        Ok(raw) => {
            if let Err(error) = write_cache(&raw).await {
                tracing::warn!(%error, "could not update models.dev cache");
            }
            (raw, CatalogSource::Live)
        }
        Err(live_error) => {
            let (raw, written_at) = read_cache().await.map_err(|cache_error| {
                format!("models.dev unavailable ({live_error}); cache unavailable ({cache_error})")
            })?;
            tracing::warn!(
                %live_error,
                %written_at,
                "models.dev could not be read; the catalog is the last fetched copy"
            );
            (
                raw,
                CatalogSource::LastFetchedCopy {
                    written_at,
                    live_error,
                },
            )
        }
    };
    let parsed = Arc::new(parse_catalog(&raw)?);
    let mut cache = MEMORY_CACHE.write().await;
    if let Some(cached) = cache.as_ref().filter(|cached| fresh(cached, ttl)) {
        return Ok(Arc::clone(&cached.snapshot));
    }
    *cache = Some(CachedSnapshot {
        loaded_at: Instant::now(),
        snapshot: Arc::clone(&parsed),
        source,
    });
    Ok(parsed)
}

/// Where the catalog in memory came from; `None` before the first load.
pub async fn source() -> Option<CatalogSource> {
    MEMORY_CACHE
        .read()
        .await
        .as_ref()
        .map(|cached| cached.source.clone())
}

async fn cached_within(ttl: Option<Duration>) -> Option<Arc<CatalogSnapshot>> {
    let cache = MEMORY_CACHE.read().await;
    cache
        .as_ref()
        .filter(|cached| fresh(cached, ttl))
        .map(|cached| Arc::clone(&cached.snapshot))
}

fn fresh(cached: &CachedSnapshot, ttl: Option<Duration>) -> bool {
    ttl.is_none_or(|ttl| cached.loaded_at.elapsed() < ttl)
}
