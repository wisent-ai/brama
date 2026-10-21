//! The lease register on disk: one file under Brama's state directory, read
//! whole and replaced atomically under one process lock, so a lease survives
//! a gateway restart and two leases taken at once cannot lose each other.

use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

use crate::subscription_dispatch::{dispatch::ranking::plan_order::shuffle_within_equal, usage};

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
    /// The agent whose proven identity took the lease, or none when the
    /// deployment console did. Only that agent, or the console, sees and
    /// ends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub taken_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at_ms: Option<i64>,
}

impl Lease {
    fn is_live(&self) -> bool {
        self.released_at_ms.is_none()
    }

    /// Whether `agent` may see and end this lease: the console (none) may
    /// act on every lease, an agent only on the ones it took.
    pub fn belongs_to(&self, agent: Option<&str>) -> bool {
        agent.is_none_or(|agent| self.agent.as_deref() == Some(agent))
    }
}

/// Why a release ended nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseRefused {
    /// The release named neither a lease nor a session.
    Invalid(String),
    /// The register could not be read or written.
    Unavailable(String),
    /// A named lease was taken by another identity; nothing was released.
    Foreign(String),
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Register {
    #[serde(default)]
    leases: Vec<Lease>,
}

/// `$BRAMA_SUBSCRIPTION_LEASES_FILE`, or `leases.json` in Brama's state
/// directory. A declaration that is not Unicode is refused by name, so leases
/// are never taken in another register than the one the operator named and
/// two gateways never lease the same member.
fn path() -> Result<PathBuf, String> {
    Ok(
        match crate::environment::declared_setting("BRAMA_SUBSCRIPTION_LEASES_FILE")? {
            Some(declared) => PathBuf::from(declared),
            None => crate::journal::state_dir().join("leases.json"),
        },
    )
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
    let path = path()?;
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
    let path = path()?;
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
    let _guard = crate::core::sync::held(&LOCK, "lease register");
    let mut register = load()?;
    let answer = apply(&mut register)?;
    persist(&register)?;
    Ok(answer)
}

/// Every live lease, newest last.
pub fn live() -> Result<Vec<Lease>, String> {
    let _guard = crate::core::sync::held(&LOCK, "lease register");
    Ok(load()?.leases.into_iter().filter(Lease::is_live).collect())
}

/// Select and persist admission against one locked view of the live register.
/// `agent` is the identity taking the lease (none for the console); a session
/// gets back only a standing lease that identity took.
pub(super) fn take(
    provider: &str,
    session_id: &str,
    holder: &str,
    agent: Option<&str>,
    entries: &[crate::gateway::broker::SubscriptionEntry],
    limit: u64,
) -> Result<super::Taken, super::Refused> {
    use super::{count, fewest_then_freshest, member_row, members, Members, Refused, Taken};

    let unavailable = |detail| Refused::Unavailable { detail };
    let _guard = crate::core::sync::held(&LOCK, "lease register");
    let mut register = load().map_err(unavailable)?;
    register.leases.retain(Lease::is_live);
    if let Some(standing) = register.leases.iter().find(|lease| {
        lease.session_id == session_id && lease.provider == provider && lease.belongs_to(agent)
    }) {
        let live_on_subscription = register
            .leases
            .iter()
            .filter(|lease| lease.subscription_id == standing.subscription_id)
            .count();
        return Ok(Taken {
            lease: standing.clone(),
            live_on_subscription: u64::try_from(live_on_subscription)
                .expect("a lease count fits in u64"),
            limit,
            new: false,
        });
    }
    let Members {
        mut usable,
        refused,
    } = members(entries, provider, &register.leases).map_err(unavailable)?;
    if usable.is_empty() {
        return Err(Refused::NoUsableMember {
            provider: provider.to_owned(),
            members: refused,
        });
    }
    usable.sort_by(fewest_then_freshest);
    let available = usable.partition_point(|(_, live)| count(live) < limit);
    shuffle_within_equal(&mut usable[..available], |(entry, live)| {
        (live.len(), usage::used_fraction(&entry.id))
    })
    .map_err(|error| unavailable(error.message))?;
    let Some((chosen, _)) = usable[..available].first() else {
        return Err(Refused::PoolFull {
            provider: provider.to_owned(),
            limit,
            members: usable
                .iter()
                .map(|(entry, live)| member_row(entry, live))
                .collect(),
        });
    };
    let taken_at_ms = now_ms().map_err(unavailable)?;
    let lease = Lease {
        id: format!("{session_id}@{provider}@{taken_at_ms}"),
        provider: provider.to_owned(),
        subscription_id: chosen.id.clone(),
        account: chosen.account.clone(),
        session_id: session_id.to_owned(),
        holder: holder.to_owned(),
        agent: agent.map(str::to_owned),
        taken_at_ms,
        released_at_ms: None,
    };
    register.leases.push(lease.clone());
    let live_on_subscription = register
        .leases
        .iter()
        .filter(|held| held.subscription_id == lease.subscription_id)
        .count();
    persist(&register).map_err(unavailable)?;
    Ok(Taken {
        lease,
        live_on_subscription: u64::try_from(live_on_subscription)
            .expect("a lease count fits in u64"),
        limit,
        new: true,
    })
}

/// Release by lease id, or every live lease of a session, as `agent` (none
/// for the console). The leases released are returned; none when nothing was
/// live. A named lease another identity took refuses the whole release, so
/// one agent never ends a session that runs on another's behalf.
pub(super) fn release(
    lease_id: Option<&str>,
    session_id: Option<&str>,
    agent: Option<&str>,
) -> Result<Vec<Lease>, ReleaseRefused> {
    let lease_id = lease_id.map(str::trim).filter(|id| !id.is_empty());
    let session_id = session_id.map(str::trim).filter(|id| !id.is_empty());
    if lease_id.is_none() && session_id.is_none() {
        return Err(ReleaseRefused::Invalid(
            "a release names the lease id or the session_id whose leases end".to_string(),
        ));
    }
    let mut foreign: Vec<String> = Vec::new();
    let released = locked(|register| {
        let released_at_ms = now_ms()?;
        let mut released = Vec::new();
        for lease in register.leases.iter_mut().filter(|lease| lease.is_live()) {
            let named = lease_id.is_some_and(|id| id == lease.id)
                || session_id.is_some_and(|id| id == lease.session_id);
            if !named {
                continue;
            }
            if !lease.belongs_to(agent) {
                foreign.push(lease.id.clone());
                continue;
            }
            lease.released_at_ms = Some(released_at_ms);
            released.push(lease.clone());
        }
        if !foreign.is_empty() {
            // Refused before anything is persisted: the register is left as
            // it was read.
            return Err(String::new());
        }
        // Released leases are history the pool document does not need;
        // the register keeps only what is live, and the journal of who ran
        // where is the holder's (Oko's sessions record).
        register.leases.retain(Lease::is_live);
        Ok(released)
    });
    // Only an agent can name a lease that is not its own; the console owns all.
    if let (Some(caller), false) = (agent, foreign.is_empty()) {
        return Err(ReleaseRefused::Foreign(format!(
            "lease {} was taken by another identity; {caller} may end only the leases it took, and nothing was released",
            foreign.join(", "),
        )));
    }
    released.map_err(ReleaseRefused::Unavailable)
}
