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
        let entries: BTreeMap<String, Vec<serde_json::Value>> = match std::fs::read(&path) {
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
        let entries = entries
            .into_iter()
            .filter(|(_, accounts)| {
                accounts.iter().all(|account| {
                    account["fact_at_ms"].is_i64() && account["discovered_at_ms"].is_i64()
                })
            })
            .map(|(source, accounts)| {
                let accounts = accounts
                    .into_iter()
                    .map(serde_json::from_value)
                    .collect::<Result<Vec<AccountObservation>, _>>()
                    .map_err(|error| format!("decode retained receipt {source}: {error}"))?;
                Ok((source, accounts))
            })
            .collect::<Result<BTreeMap<_, _>, String>>()?;
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
