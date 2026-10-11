//! Configuration reads name the field or key file that refused the client.
use std::path::{Path, PathBuf};

use super::workload_key::read_owner_key;
use super::{CapabilityClient, CapabilityError};

fn absolute_path(name: &str) -> Result<PathBuf, CapabilityError> {
    let value = std::env::var_os(name)
        .ok_or_else(|| CapabilityError::InvalidConfiguration(format!("{name} is not set")))?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(CapabilityError::InvalidConfiguration(format!(
            "{name} must be an absolute path; observed {}",
            path.display()
        )));
    }
    Ok(path)
}

impl CapabilityClient {
    pub fn from_env() -> Result<Self, CapabilityError> {
        let socket = absolute_path("SKARBIEC_CAP_SOCKET")?;
        let workload_id = std::env::var("SKARBIEC_WORKLOAD_ID").map_err(|error| {
            CapabilityError::InvalidConfiguration(format!("read SKARBIEC_WORKLOAD_ID: {error}"))
        })?;
        let key_path = absolute_path("SKARBIEC_WORKLOAD_SIGNING_KEY_FILE")?;
        Self::new(socket, workload_id, &key_path)
    }

    pub fn new(
        socket: PathBuf,
        workload_id: String,
        signing_key_file: &Path,
    ) -> Result<Self, CapabilityError> {
        if !socket.is_absolute() {
            return Err(CapabilityError::InvalidConfiguration(format!(
                "capability socket must be absolute; observed {}",
                socket.display()
            )));
        }
        if workload_id.is_empty() {
            return Err(CapabilityError::InvalidConfiguration(
                "SKARBIEC_WORKLOAD_ID must not be empty".to_string(),
            ));
        }
        let signing_key = read_owner_key(signing_key_file)?;
        Ok(Self {
            socket,
            workload_id,
            signing_key,
        })
    }
}
