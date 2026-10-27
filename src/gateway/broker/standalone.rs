//! The credential map a standalone desktop installation holds.
//!
//! A managed installation redeems every credential from the broker, and this
//! is what replaces that for a desktop launch that has no Skarbiec at all: one
//! map installed before the server starts, one for provider credentials and
//! one for the subscriptions donated into this process or renewed by it. The
//! values live in zeroizing memory, and the presence of the provider map is
//! what every caller above tests to know which world it is in. A launcher
//! that names an owner-only grants file (`standalone/grants.rs`) keeps the
//! subscription grants across restarts: they are read from it when the
//! provider map is installed and written back to it on every donation,
//! renewal and removal, so a renewal that rotates a refresh token is not lost
//! when the process ends. Without that file they live only as long as the
//! process does.

mod grants;

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock, RwLock};

use zeroize::{Zeroize, Zeroizing};

use super::subscription_resource;
use crate::capability::Secret;

type LocalCredential = Zeroizing<Vec<u8>>;
type LocalCredentialMap = HashMap<String, LocalCredential>;

static LOCAL_PROVIDER_CREDENTIALS: OnceLock<RwLock<LocalCredentialMap>> = OnceLock::new();
pub(super) static LOCAL_SUBSCRIPTION_CREDENTIALS: LazyLock<RwLock<LocalCredentialMap>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Install direct provider credentials supplied by the standalone desktop
/// launcher. The JSON input is consumed and zeroized; plaintext then remains
/// only in zeroizing process memory for this server lifetime.
pub fn install_local_provider_credentials(encoded: &mut String) -> Result<(), String> {
    let parsed: Result<HashMap<String, String>, _> = serde_json::from_str(encoded);
    encoded.zeroize();
    let mut parsed =
        parsed.map_err(|error| format!("local provider credentials are invalid: {error}"))?;
    if parsed
        .iter()
        .any(|(provider, value)| provider.trim().is_empty() || value.is_empty())
    {
        parsed.values_mut().for_each(Zeroize::zeroize);
        return Err("local provider names and credentials must be non-empty".to_owned());
    }
    let mut credentials = HashMap::with_capacity(parsed.len());
    for (provider, mut value) in parsed {
        credentials.insert(
            provider.trim().to_owned(),
            Zeroizing::new(value.as_bytes().to_vec()),
        );
        value.zeroize();
    }
    if LOCAL_PROVIDER_CREDENTIALS
        .set(RwLock::new(credentials))
        .is_err()
    {
        return Err("local provider credentials were already installed".to_owned());
    }
    let mut held = crate::core::sync::write_held(
        &LOCAL_SUBSCRIPTION_CREDENTIALS,
        "local subscription credentials",
    );
    grants::load(&mut held)
}

pub fn local_provider_credentials_enabled() -> bool {
    LOCAL_PROVIDER_CREDENTIALS.get().is_some()
}

fn local_provider_store() -> Result<&'static RwLock<LocalCredentialMap>, String> {
    LOCAL_PROVIDER_CREDENTIALS
        .get()
        .ok_or_else(|| "standalone credential store is not enabled".to_owned())
}

pub fn local_provider_names() -> Result<Vec<String>, String> {
    let credentials =
        crate::core::sync::read_held(local_provider_store()?, "local provider credentials");
    let mut names = credentials.keys().cloned().collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

pub fn put_local_provider_credential(provider: &str, credential: &str) -> Result<(), String> {
    let provider = provider.trim();
    if provider.is_empty() || credential.is_empty() {
        return Err("provider and credential must contain valid values".to_owned());
    }
    let mut credentials =
        crate::core::sync::write_held(local_provider_store()?, "local provider credentials");
    credentials.insert(
        provider.to_owned(),
        Zeroizing::new(credential.as_bytes().to_vec()),
    );
    Ok(())
}

pub fn remove_local_provider_credential(provider: &str) -> Result<bool, String> {
    let mut credentials =
        crate::core::sync::write_held(local_provider_store()?, "local provider credentials");
    Ok(credentials.remove(provider).is_some())
}

pub(super) fn local_provider_credential(provider: &str) -> Result<Secret, String> {
    crate::core::sync::read_held(local_provider_store()?, "local provider credentials")
        .get(provider)
        .map(|value| Secret::from_bytes(value.as_slice().to_vec()))
        .ok_or_else(|| format!("standalone credential store has no credential for `{provider}`"))
}

/// Hold `secret` as the grant of `item_id` and keep it in the launcher's
/// grants file. A refused save leaves the held grant as it was, so the
/// process never uses a grant its caller was told was not kept.
pub(super) fn put_local_subscription_credential(
    item_id: &str,
    secret: &[u8],
) -> Result<(), String> {
    let mut held = crate::core::sync::write_held(
        &LOCAL_SUBSCRIPTION_CREDENTIALS,
        "local subscription credentials",
    );
    let previous = held.insert(item_id.to_owned(), Zeroizing::new(secret.to_vec()));
    grants::save(&held).inspect_err(|_refused| match previous {
        Some(kept) => {
            held.insert(item_id.to_owned(), kept);
        }
        None => {
            held.remove(item_id);
        }
    })
}

/// Drop the grant of `provider`'s `subscription_id` and keep the grants
/// file without it. A refused save puts the grant back, so the process does
/// not stop using a grant the file still holds.
pub fn remove_donated_credential(provider: &str, subscription_id: &str) -> Result<(), String> {
    if !local_provider_credentials_enabled() {
        return Ok(());
    }
    let mut held = crate::core::sync::write_held(
        &LOCAL_SUBSCRIPTION_CREDENTIALS,
        "local subscription credentials",
    );
    let key = subscription_resource(provider, subscription_id);
    let removed = held.remove(&key);
    grants::save(&held).inspect_err(|_refused| {
        if let Some(kept) = removed {
            held.insert(key, kept);
        }
    })
}
