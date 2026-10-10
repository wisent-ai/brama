//! Keep account facts until exact-account vault reconciliation succeeds.

use super::harness::AccountObservation;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

pub struct Pending {
    path: PathBuf,
    pub accounts: Vec<AccountObservation>,
    _lock: File,
}

impl Pending {
    pub fn open() -> Result<Self, String> {
        let directory = crate::journal::state_dir().join("account-discovery");
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("create pending account discovery directory: {error}"))?;
        let lock = OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join("pending.lock"))
            .map_err(|error| format!("open pending account discovery lock: {error}"))?;
        lock.try_lock().map_err(|error| {
            format!("account reconciliation already running or lock unavailable: {error}")
        })?;
        let path = directory.join("pending.json");
        let accounts = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                format!(
                    "decode pending account discovery {}: {error}",
                    path.display()
                )
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(format!(
                    "read pending account discovery {}: {error}",
                    path.display()
                ))
            }
        };
        Ok(Self {
            path,
            accounts,
            _lock: lock,
        })
    }

    pub fn retain(&mut self, observations: &[AccountObservation]) -> Result<(), String> {
        if observations.is_empty() {
            return Ok(());
        }
        for observation in observations {
            let existing = self.accounts.iter_mut().find(|existing| {
                existing.provider == observation.provider
                    && existing.account.eq_ignore_ascii_case(&observation.account)
                    && existing.source == observation.source
            });
            match existing {
                Some(existing) => {
                    existing.discovered_at_ms =
                        existing.discovered_at_ms.min(observation.discovered_at_ms);
                    existing.observed_at_ms =
                        existing.observed_at_ms.max(observation.observed_at_ms);
                    if observation.plan.is_some()
                        && (existing.plan.is_none()
                            || observation.fact_at_ms >= existing.fact_at_ms)
                    {
                        existing.plan = observation.plan.clone();
                        existing.fact_at_ms = observation.fact_at_ms;
                    }
                }
                None => self.accounts.push(observation.clone()),
            }
        }
        self.persist()
    }

    pub fn reconciled(&mut self) -> Result<(), String> {
        self.accounts.clear();
        self.persist()
    }

    fn persist(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec(&self.accounts)
            .map_err(|error| format!("encode pending account discovery: {error}"))?;
        let pending = self.path.with_extension("writing");
        let mut file = File::create(&pending).map_err(|error| {
            format!(
                "create pending account discovery {}: {error}",
                pending.display()
            )
        })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| std::fs::rename(&pending, &self.path))
            .map_err(|error| {
                format!(
                    "persist pending account discovery {}: {error}",
                    self.path.display()
                )
            })?;
        let directory = self
            .path
            .parent()
            .expect("pending discovery file has a parent");
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                format!(
                    "persist account discovery directory {}: {error}",
                    directory.display()
                )
            })
    }
}
