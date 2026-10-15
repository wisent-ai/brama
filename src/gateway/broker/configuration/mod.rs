//! Optional declarations may be absent; malformed declarations are refusals.
use std::collections::HashMap;

use crate::capability::{CapabilityClient, CapabilityError};

pub(super) fn capability_map(name: &str) -> Result<HashMap<String, String>, String> {
    let Some(encoded) = crate::environment::declared_text(name)
        .map_err(|refusal| format!("read capability declaration {name}: {refusal}"))?
    else {
        return Ok(HashMap::new());
    };
    serde_json::from_str(&encoded)
        .map_err(|error| format!("decode capability declaration {name}: {error}"))
}

pub(super) fn configured_capability(name: &str, key: &str) -> Result<Option<String>, String> {
    Ok(capability_map(name)?.remove(key))
}

pub(super) fn client() -> Result<CapabilityClient, CapabilityError> {
    CapabilityClient::from_env()
}
