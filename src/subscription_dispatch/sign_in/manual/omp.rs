//! The grants `omp` holds, read from its own store.
//!
//! `omp` keeps every credential it signed in under `~/.omp/agent/agent.db`,
//! table `auth_credentials`, one row per account, the grant as a JSON
//! document in `data` with its own field names - `access`, `refresh`,
//! `expires` in milliseconds, `accountId`, `email` - whatever the provider.
//! Brama reads the store read-only and rewrites each grant into the shape
//! its refresh path reads for that provider.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use zeroize::Zeroizing;

use super::harness::{millis, Harness, HeldGrant};

/// The harness's name for each provider Brama knows.
fn harness_provider(provider: &str) -> Option<&'static str> {
    match provider {
        "claude-code" => Some("anthropic"),
        "codex" => Some("openai-codex"),
        "kimi" => Some("kimi-code"),
        _ => None,
    }
}

/// Every enabled grant the store holds for `provider`, newest first, in
/// Brama's shape. A store that is not there holds nothing.
pub fn held(store: &Path, provider: &'static str) -> Result<Vec<HeldGrant>, String> {
    let Some(harness) = harness_provider(provider) else {
        return Ok(Vec::new());
    };
    if !store.is_file() {
        return Ok(Vec::new());
    }
    let connection = Connection::open_with_flags(store, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("omp's store {} cannot be opened: {error}", store.display()))?;
    let mut statement = connection
        .prepare(
            "SELECT data FROM auth_credentials \
             WHERE provider = ?1 AND credential_type = 'oauth' AND disabled_cause IS NULL \
             ORDER BY updated_at DESC",
        )
        .map_err(|error| {
            format!(
                "omp's store {} is not the store Brama knows: {error}",
                store.display()
            )
        })?;
    let rows = statement
        .query_map([harness], |row| row.get::<_, String>(0))
        .map_err(|error| format!("reading omp's store: {error}"))?;
    let mut found = Vec::new();
    for row in rows {
        let data = Zeroizing::new(row.map_err(|error| format!("reading omp's store: {error}"))?);
        let value: Value = serde_json::from_str(&data)
            .map_err(|_| "an omp credential is not the JSON document omp writes".to_owned())?;
        let field = |name: &str| value.get(name).and_then(Value::as_str).map(str::to_owned);
        let (Some(access), Some(refresh)) = (field("access"), field("refresh")) else {
            continue;
        };
        let expires_at_ms = milliseconds(
            value
                .get("expires")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
        );
        let account = field("email").or_else(|| field("accountId"));
        let document = document(
            provider,
            &access,
            &refresh,
            expires_at_ms,
            field("accountId").as_deref(),
        );
        found.push(HeldGrant {
            harness: Harness::Omp,
            provider,
            account,
            expires_at_ms: Some(expires_at_ms),
            document: Zeroizing::new(document.to_string()),
        });
    }
    Ok(found)
}

/// omp's grant in the shape Brama's refresh path reads for the provider -
/// the same three shapes a Weles sign-in writes.
fn document(
    provider: &str,
    access: &str,
    refresh: &str,
    expires_at_ms: i64,
    account_id: Option<&str>,
) -> Value {
    match provider {
        "claude-code" => json!({
            "claudeAiOauth": {
                "accessToken": access,
                "refreshToken": refresh,
                "expiresAt": expires_at_ms,
                "scopes": super::claude_scopes(),
            }
        }),
        "codex" => json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": access,
                "refresh_token": refresh,
                "account_id": account_id.unwrap_or_default(),
            },
            "last_refresh": chrono::Utc::now().to_rfc3339(),
        }),
        _ => json!({
            "access_token": access,
            "refresh_token": refresh,
            "expires_at": expires_at_ms / millis(),
        }),
    }
}

/// omp writes its expiry in milliseconds; a value below 10^11 (the year 5138 in
/// seconds) can only be seconds, and is read as such.
const SECONDS_BOUNDARY: i64 = 100_000_000_000;

fn milliseconds(expires: i64) -> i64 {
    let seconds_boundary = SECONDS_BOUNDARY;
    if expires < seconds_boundary {
        expires * millis()
    } else {
        expires
    }
}
