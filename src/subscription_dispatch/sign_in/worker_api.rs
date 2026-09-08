//! Where Weles's worker API is, what admits Brama to it, and what it says
//! about itself before a sign-in is requested.
//!
//! This is separate because none of it is about signing an account in. It is
//! the four answers a sign-in needs before it can ask for anything: which host
//! serves Weles right now, the bearer that admits Brama to the
//! reauthentication route, how long one exchange may hold the connection, and
//! whether the service on that port is really Weles. Each changes for its own
//! reason -- a redeployment, a Skarbiec rotation, a longer consent screen, a
//! new health field -- and none of them changes when the sign-in itself does.

use std::time::Duration;

use serde_json::Value;

/// Resolve Weles from Stado at the moment a sign-in needs it. Placement can
/// change while Brama keeps serving model traffic; baking loopback into the
/// launcher made the renewal path silently keep the old host forever.
pub(super) async fn worker_api_base() -> Result<String, String> {
    if let Ok(configured) = std::env::var("BRAMA_WELES_URL") {
        let configured = configured.trim();
        if !configured.is_empty() {
            reqwest::Url::parse(configured)
                .map_err(|error| format!("BRAMA_WELES_URL is invalid: {error}"))?;
            return Ok(configured.trim_end_matches('/').to_string());
        }
    }
    let stado = std::env::var("BRAMA_STADO_BIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            std::path::PathBuf::from(home)
                .join(".stado")
                .join("bin")
                .join("stado")
        });
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(&stado)
            .kill_on_drop(true)
            .args([
                "service",
                "directory",
                "connect",
                "weles-admission",
                "--no-verify",
                "--json",
            ])
            .output(),
    )
    .await
    .map_err(|_| {
        format!(
            "Stado weles-admission lookup through {} timed out after 30 seconds",
            stado.display()
        )
    })?
    .map_err(|error| {
        format!(
            "cannot resolve weles-admission through {}: {error}",
            stado.display()
        )
    })?;
    if !output.status.success() {
        let detail: String = String::from_utf8_lossy(&output.stderr)
            .trim()
            .chars()
            .take(500)
            .collect();
        return Err(format!(
            "Stado could not resolve weles-admission: {}",
            if detail.is_empty() {
                output.status.to_string()
            } else {
                detail
            }
        ));
    }
    let document: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Stado weles-admission answer is not JSON: {error}"))?;
    let url = document
        .get("url")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Stado weles-admission answer carries no URL".to_string())?;
    reqwest::Url::parse(url)
        .map_err(|error| format!("Stado returned an invalid weles-admission URL: {error}"))?;
    Ok(url.trim_end_matches('/').to_string())
}

fn env_or(key: &str, default: &str) -> String {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => default.to_string(),
    }
}

/// How long one HTTP exchange with Weles may take. The reauth call holds the
/// connection for the length of the sign-in, so this must exceed the login
/// budget.
pub(super) fn transport_timeout_seconds() -> u64 {
    env_or("BRAMA_SIGN_IN_TRANSPORT_TIMEOUT_SECONDS", "1200")
        .parse()
        .unwrap_or(1200)
}

/// Brama's Weles admission credential. The launcher acquires this field from
/// `brama-weles-reauth` through the entitlements router at every service start.
/// It is deliberately distinct from Weles's general worker API token.
pub(super) fn worker_api_token() -> Result<String, String> {
    let token = std::env::var("BRAMA_WELES_REAUTH_TOKEN")
        .unwrap_or_default()
        .trim()
        .to_string();
    if token.is_empty() {
        Err(
            "BRAMA_WELES_REAUTH_TOKEN is unavailable; Brama must acquire \
             brama-weles-reauth/token from Skarbiec at startup"
                .into(),
        )
    } else {
        Ok(token)
    }
}

/// Weles's own health answer, which advertises the selector contract and the
/// sign-in rows it holds. `error_for_status` matters: a healthy exit from an
/// unrelated service that happens to hold this port must not send a sign-in
/// request nobody serves.
pub(super) async fn read_health(client: &reqwest::Client, base: &str) -> Result<Value, String> {
    let response = client
        .get(format!("{base}/healthz"))
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("Weles health request at {base}/healthz failed: {error}"))?;
    response
        .json()
        .await
        .map_err(|error| format!("Weles health answer is not JSON: {error}"))
}
