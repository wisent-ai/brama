//! The Claude grants the operator's harness already holds.
//!
//! `omp` keeps every credential it signed in under `~/.omp/agent/agent.db`,
//! table `auth_credentials`, one row per account, the grant as a JSON
//! document in `data`. On 2026-09-13 the operator asked why Brama did not
//! simply take those tokens instead of asking him to log in again: the
//! harness beside Brama had been signed in for a day while the pool held
//! nothing. So Brama reads the harness's store - read-only, never a write -
//! and takes the grant of the account the operator names.

use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use zeroize::Zeroizing;

use super::grant::Grant;

/// Where the harness keeps its credentials, beside its transcripts.
pub fn default_store() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.omp/agent/agent.db")
}

/// The harness's name for the provider Brama calls `claude-code`.
fn harness_provider(provider: &str) -> Option<&'static str> {
    match provider {
        "claude-code" => Some("anthropic"),
        _ => None,
    }
}

/// One account the harness holds a live grant for.
pub struct HarnessAccount {
    pub email: String,
    pub organization: Option<String>,
    pub grant: Grant,
}

/// Every enabled grant the harness holds for `provider`, newest first.
pub fn accounts(store: &str, provider: &str) -> Result<Vec<HarnessAccount>, String> {
    let harness = harness_provider(provider)
        .ok_or_else(|| format!("the harness holds no grants Brama can take for `{provider}`; claude-code is the one it can"))?;
    let connection = Connection::open_with_flags(store, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("the harness store {store} cannot be opened: {error}"))?;
    let mut statement = connection
        .prepare(
            "SELECT data FROM auth_credentials \
             WHERE provider = ?1 AND credential_type = 'oauth' AND disabled_cause IS NULL \
             ORDER BY updated_at DESC",
        )
        .map_err(|error| {
            format!("the harness store {store} is not the store Brama knows: {error}")
        })?;
    let rows = statement
        .query_map([harness], |row| row.get::<_, String>(0))
        .map_err(|error| format!("reading the harness store: {error}"))?;
    let mut found = Vec::new();
    for row in rows {
        let data =
            Zeroizing::new(row.map_err(|error| format!("reading the harness store: {error}"))?);
        let value: Value = serde_json::from_str(&data).map_err(|_| {
            "a harness credential is not the JSON document the harness writes".to_owned()
        })?;
        let field = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_owned);
        let (Some(email), Some(access), Some(refresh)) =
            (field("email"), field("access"), field("refresh"))
        else {
            continue;
        };
        found.push(HarnessAccount {
            grant: Grant {
                access_token: Zeroizing::new(access),
                refresh_token: Zeroizing::new(refresh),
                expires_at_ms: milliseconds(
                    value
                        .get("expires")
                        .and_then(Value::as_i64)
                        .unwrap_or_default(),
                ),
                account: Some(email.clone()),
            },
            organization: field("orgName"),
            email,
        });
    }
    Ok(found)
}

/// The harness writes its expiry in milliseconds; a value that small can only
/// be seconds, and is read as such.
fn milliseconds(expires: i64) -> i64 {
    let seconds_boundary: i64 = "100000000000".parse().expect("valid boundary");
    if expires < seconds_boundary {
        expires
            * "1000"
                .parse::<i64>()
                .expect("valid milliseconds per second")
    } else {
        expires
    }
}

/// The one account the operator asked for, or the only one, or a refusal
/// that names the choices.
pub fn account(store: &str, provider: &str, email: Option<&str>) -> Result<HarnessAccount, String> {
    let mut found = accounts(store, provider)?;
    if let Some(email) = email {
        return found
            .into_iter()
            .find(|account| account.email.eq_ignore_ascii_case(email.trim()))
            .ok_or_else(|| format!("the harness holds no enabled `{provider}` grant for {email}"));
    }
    match found.len() {
        0 => Err(format!("the harness holds no enabled `{provider}` grant; sign it in there first, or sign in by hand")),
        1 => Ok(found.remove(0)),
        _ => Err(format!(
            "the harness holds {} enabled `{provider}` grants; name one with --account: {}",
            found.len(),
            found.iter().map(|account| account.email.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}
