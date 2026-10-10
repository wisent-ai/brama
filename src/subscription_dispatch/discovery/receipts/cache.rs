//! Cache successful message interpretations, including confirmed non-receipts.

use super::super::harness::AccountObservation;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct Cache {
    path: PathBuf,
    pub entries: BTreeMap<String, Vec<AccountObservation>>,
    _lock: std::fs::File,
}

impl Cache {
    pub fn open() -> Result<Self, String> {
        let directory = crate::journal::state_dir().join("account-discovery");
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("create receipt discovery state: {error}"))?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join("receipts.lock"))
            .map_err(|error| format!("open receipt discovery lock: {error}"))?;
        lock.try_lock().map_err(|error| {
            format!("receipt discovery already running or lock unavailable: {error}")
        })?;
        let path = directory.join("receipts.json");
        let entries = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                format!(
                    "decode retained receipt discovery {}: {error}",
                    path.display()
                )
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => {
                return Err(format!(
                    "read retained receipt discovery {}: {error}",
                    path.display()
                ))
            }
        };
        Ok(Self {
            path,
            entries,
            _lock: lock,
        })
    }

    pub fn store(
        &mut self,
        source: String,
        accounts: Vec<AccountObservation>,
    ) -> Result<(), String> {
        self.entries.insert(source, accounts);
        let bytes = serde_json::to_vec(&self.entries)
            .map_err(|error| format!("encode receipt discovery cache: {error}"))?;
        let pending = self.path.with_extension("pending");
        std::fs::write(&pending, bytes)
            .and_then(|()| std::fs::rename(&pending, &self.path))
            .map_err(|error| format!("persist receipt discovery {}: {error}", self.path.display()))
    }
}
