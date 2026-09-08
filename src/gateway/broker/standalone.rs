//! The credential map a standalone desktop installation holds in memory.
//!
//! A managed installation redeems every credential from the broker, and this
//! is what replaces that for a desktop launch that has no Skarbiec at all: one
//! map installed before the server starts, one for provider credentials and
//! one for the subscriptions donated into this process. It is a separate
//! subject because these values are never read from anywhere, never rotated
//! and never written back -- they only live as long as the process does, in
//! zeroizing memory, and the presence of the provider map is what every
//! caller above tests to know which world it is in.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

use zeroize::{Zeroize, Zeroizing};

use super::subscription_resource;
use crate::capability::Secret;

type LocalCredential = Zeroizing<Vec<u8>>;
type LocalCredentialMap = HashMap<String, LocalCredential>;

pub(super) static LOCAL_PROVIDER_CREDENTIALS: LazyLock<RwLock<Option<LocalCredentialMap>>> =
    LazyLock::new(|| RwLock::new(None));
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
    let mut installed = LOCAL_PROVIDER_CREDENTIALS
        .write()
        .map_err(|_| "local provider credential lock is poisoned".to_owned())?;
    if installed.is_some() {
        return Err("local provider credentials were already installed".to_owned());
    }
    *installed = Some(credentials);
    Ok(())
}

pub fn local_provider_credentials_enabled() -> bool {
    LOCAL_PROVIDER_CREDENTIALS
        .read()
        .is_ok_and(|credentials| credentials.is_some())
}

pub fn local_provider_names() -> Result<Vec<String>, String> {
    let credentials = LOCAL_PROVIDER_CREDENTIALS
        .read()
        .map_err(|_| "local provider credential lock is poisoned".to_owned())?;
    let mut names = credentials
        .as_ref()
        .ok_or_else(|| "standalone credential store is not enabled".to_owned())?
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    names.sort();
    Ok(names)
}

pub fn put_local_provider_credential(provider: &str, credential: &str) -> Result<(), String> {
    let provider = provider.trim();
    if provider.is_empty() || credential.is_empty() || credential.chars().count() > 8000 {
        return Err("provider and credential must contain valid values".to_owned());
    }
    let mut credentials = LOCAL_PROVIDER_CREDENTIALS
        .write()
        .map_err(|_| "local provider credential lock is poisoned".to_owned())?;
    let credentials = credentials
        .as_mut()
        .ok_or_else(|| "standalone credential store is not enabled".to_owned())?;
    credentials.insert(
        provider.to_owned(),
        Zeroizing::new(credential.as_bytes().to_vec()),
    );
    Ok(())
}

pub fn remove_local_provider_credential(provider: &str) -> Result<bool, String> {
    let mut credentials = LOCAL_PROVIDER_CREDENTIALS
        .write()
        .map_err(|_| "local provider credential lock is poisoned".to_owned())?;
    let credentials = credentials
        .as_mut()
        .ok_or_else(|| "standalone credential store is not enabled".to_owned())?;
    Ok(credentials.remove(provider).is_some())
}

pub(super) fn local_provider_credential(provider: &str) -> Option<Secret> {
    LOCAL_PROVIDER_CREDENTIALS
        .read()
        .ok()?
        .as_ref()?
        .get(provider)
        .map(|value| Secret::from_bytes(value.as_slice().to_vec()))
}

pub(super) fn put_local_subscription_credential(
    item_id: &str,
    secret: &[u8],
) -> Result<(), String> {
    LOCAL_SUBSCRIPTION_CREDENTIALS
        .write()
        .map_err(|_| "local subscription credential lock is poisoned".to_owned())?
        .insert(item_id.to_owned(), Zeroizing::new(secret.to_vec()));
    Ok(())
}

pub fn remove_donated_credential(provider: &str, subscription_id: &str) -> Result<(), String> {
    if !local_provider_credentials_enabled() {
        return Ok(());
    }
    LOCAL_SUBSCRIPTION_CREDENTIALS
        .write()
        .map_err(|_| "local subscription credential lock is poisoned".to_owned())?
        .remove(&subscription_resource(provider, subscription_id));
    Ok(())
}
