//! What a standing credential call is measured by: the refresh interval this
//! host's Stado resolver declares, else the operator's own declaration. A
//! gateway Stado does not place, such as a standalone or Desktop runtime,
//! has no resolver to declare one; without any measure a call that never
//! answers is listed but never ended, and `/readyz` says so.

use std::path::PathBuf;

use serde_json::{json, Value};

/// The operator's declaration of how long a credential call may stand, read
/// only where this host's resolver declares no refresh interval.
const STANDING_LIMIT_ENV: &str = "BRAMA_CREDENTIAL_CALL_STANDING_SECONDS";

/// How long a credential call may stand, and who declared it.
pub(super) struct Measure {
    pub(super) seconds: u64,
    pub(super) declared_by: String,
}

/// Where Stado's resolver keeps its state on this machine: Stado's own
/// contract, overridable the way Stado's readers override it. A process
/// with neither the override nor a home cannot say where that is.
pub(in crate::core::server) fn resolver_state_file() -> Result<PathBuf, String> {
    if let Some(path) =
        std::env::var_os("STADO_RESOLVER_STATE_FILE").filter(|path| !path.is_empty())
    {
        return Ok(PathBuf::from(path));
    }
    let home = crate::journal::home().map_err(|cause| {
        format!(
            "STADO_RESOLVER_STATE_FILE is not set and {cause}, so nothing names where this \
             host's Stado resolver keeps its state"
        )
    })?;
    Ok(home.join(".stado").join("resolver-state.json"))
}

/// One JSON document of the resolver's: `None` when the file is absent, an
/// error naming the file when it cannot be read or is not JSON.
fn resolver_document(path: &PathBuf) -> Result<Option<Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| format!("{} is not JSON: {error}", path.display()))
}

/// The refresh interval, in seconds, this host's resolver declares: the
/// target its state names, looked up in Stado's last-known-good registry
/// copy beside it. `None` when either file is absent or the target declares
/// no interval; a file that cannot be read, or a state and registry that do
/// not name the target, is an error naming which.
fn declared_refresh() -> Result<Option<u64>, String> {
    let state_file = resolver_state_file()?;
    let Some(state) = resolver_document(&state_file)? else {
        return Ok(None);
    };
    let target = state["target"]
        .as_str()
        .ok_or_else(|| format!("{} names no target text", state_file.display()))?;
    let registry_file = state_file
        .with_file_name("cache")
        .join("registry-last-good.json");
    let Some(registry) = resolver_document(&registry_file)? else {
        return Ok(None);
    };
    let targets = registry["targets"]
        .as_array()
        .ok_or_else(|| format!("{} holds no targets list", registry_file.display()))?;
    let entry = targets
        .iter()
        .find(|entry| entry["name"].as_str() == Some(target))
        .ok_or_else(|| {
            format!(
                "{} lists no target {target}, the one {} names",
                registry_file.display(),
                state_file.display()
            )
        })?;
    Ok(entry["service_resolver"]["refresh_seconds"].as_u64())
}

/// The resolver's refresh interval, else the operator's
/// [`STANDING_LIMIT_ENV`]; `None` when neither declares one, an error when
/// either declaration cannot be read.
pub(super) fn declared_measure() -> Result<Option<Measure>, String> {
    if let Some(seconds) = declared_refresh()? {
        return Ok(Some(Measure {
            seconds,
            declared_by: "the refresh interval this host's resolver declares".to_string(),
        }));
    }
    match crate::types::declared_age(STANDING_LIMIT_ENV)? {
        Some(age) if age.is_zero() => Err(format!(
            "{STANDING_LIMIT_ENV} must be at least one second"
        )),
        Some(age) => Ok(Some(Measure {
            seconds: age.as_secs(),
            declared_by: STANDING_LIMIT_ENV.to_string(),
        })),
        None => Ok(None),
    }
}

/// What `/readyz` says standing calls are measured by: the seconds and who
/// declared them, the sentence saying no call is ever ended, or why the
/// declaration cannot be read.
pub(in crate::core::server) fn measure_document() -> Value {
    match declared_measure() {
        Ok(Some(measure)) => json!({
            "seconds": measure.seconds,
            "declared_by": measure.declared_by,
        }),
        Ok(None) => json!({
            "unbounded": format!(
                "neither this host's resolver nor {STANDING_LIMIT_ENV} declares how long a credential call may stand, so a call that never answers is listed in standing_calls and never ended"
            ),
        }),
        Err(cause) => json!({ "unreadable": cause }),
    }
}
