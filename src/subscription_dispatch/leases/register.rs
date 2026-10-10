//! The lease register on disk: one file under Brama's state directory, read
//! whole and replaced atomically under one process lock, so a lease survives
//! a gateway restart and two leases taken at once cannot lose each other.

use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

/// One session's claim on one subscription.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Lease {
    pub id: String,
    pub provider: String,
    pub subscription_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub session_id: String,
    /// The program that took the lease for the session (Oko, a desktop).
    pub holder: String,
    pub taken_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at_ms: Option<i64>,
}

impl Lease {
    fn is_live(&self) -> bool {
        self.released_at_ms.is_none()
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Register {
    #[serde(default)]
    leases: Vec<Lease>,
}

/// `$BRAMA_SUBSCRIPTION_LEASES_FILE`, or `leases.json` in Brama's state
/// directory.
fn path() -> PathBuf {
    match std::env::var("BRAMA_SUBSCRIPTION_LEASES_FILE") {
        Ok(declared) if !declared.trim().is_empty() => PathBuf::from(declared.trim()),
        _ => crate::journal::state_dir().join("leases.json"),
    }
}

static LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn now_ms() -> Result<i64, String> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| format!("the clock stands before the epoch: {error}"))?;
    i64::try_from(elapsed.as_millis())
        .map_err(|error| format!("the clock's milliseconds do not fit a lease instant: {error}"))
}

fn load() -> Result<Register, String> {
    let path = path();
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            format!(
                "the lease register {} is not readable JSON ({error}); repair or remove it, nothing is leased until it reads",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Register::default()),
        Err(error) => Err(format!(
            "the lease register {} cannot be read: {error}",
            path.display()
        )),
    }
}

fn persist(register: &Register) -> Result<(), String> {
    let path = path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "the lease register's directory {} cannot be made: {error}",
                parent.display()
            )
        })?;
    }
    let staged = path.with_extension("json.staging");
    let text = serde_json::to_vec_pretty(register)
        .map_err(|error| format!("the lease register cannot be encoded: {error}"))?;
    std::fs::write(&staged, text).map_err(|error| {
        format!(
            "the lease register {} cannot be staged: {error}",
            staged.display()
        )
    })?;
    std::fs::rename(&staged, &path).map_err(|error| {
        format!(
            "the lease register {} cannot replace {}: {error}",
            staged.display(),
            path.display()
        )
    })
}

fn locked<T>(apply: impl FnOnce(&mut Register) -> Result<T, String>) -> Result<T, String> {
    let _guard = LOCK
        .lock()
        .map_err(|error| format!("the lease register lock is poisoned: {error}"))?;
    let mut register = load()?;
    let answer = apply(&mut register)?;
    persist(&register)?;
    Ok(answer)
}

/// Every live lease, newest last.
pub fn live() -> Result<Vec<Lease>, String> {
    let _guard = LOCK
        .lock()
        .map_err(|error| format!("the lease register lock is poisoned: {error}"))?;
    Ok(load()?.leases.into_iter().filter(Lease::is_live).collect())
}

/// Record one lease. The id is the lease's own: the session and the instant
/// it was taken at, so two leases of one session on two providers differ.
pub(super) fn take(
    provider: &str,
    subscription_id: &str,
    account: Option<&str>,
    session_id: &str,
    holder: &str,
) -> Result<Lease, String> {
    locked(|register| {
        let taken_at_ms = now_ms()?;
        let lease = Lease {
            id: format!("{session_id}@{provider}@{taken_at_ms}"),
            provider: provider.to_string(),
            subscription_id: subscription_id.to_string(),
            account: account.map(str::to_string),
            session_id: session_id.to_string(),
            holder: holder.to_string(),
            taken_at_ms,
            released_at_ms: None,
        };
        register.leases.push(lease.clone());
        Ok(lease)
    })
}

/// Release by lease id, or every live lease of a session. The leases
/// released are returned; none when nothing was live.
pub(super) fn release(
    lease_id: Option<&str>,
    session_id: Option<&str>,
) -> Result<Vec<Lease>, String> {
    let lease_id = lease_id.map(str::trim).filter(|id| !id.is_empty());
    let session_id = session_id.map(str::trim).filter(|id| !id.is_empty());
    if lease_id.is_none() && session_id.is_none() {
        return Err("a release names the lease id or the session_id whose leases end".to_string());
    }
    locked(|register| {
        let released_at_ms = now_ms()?;
        let mut released = Vec::new();
        for lease in register.leases.iter_mut().filter(|lease| lease.is_live()) {
            let named = lease_id.is_some_and(|id| id == lease.id)
                || session_id.is_some_and(|id| id == lease.session_id);
            if named {
                lease.released_at_ms = Some(released_at_ms);
                released.push(lease.clone());
            }
        }
        // Released leases are history the pool document does not need;
        // the register keeps only what is live, and the journal of who ran
        // where is the holder's (Oko's sessions record).
        register.leases.retain(Lease::is_live);
        Ok(released)
    })
}
