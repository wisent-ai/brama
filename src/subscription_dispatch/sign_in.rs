//! One operator-driven sign-in of a provider subscription account.
//!
//! The gateway cannot repair a `needs_reauthorization` credential by itself: a
//! grant the provider disowned is only replaced by a real sign-in, and for
//! claude and kimi no local CLI can perform one. Weles owns that sign-in,
//! drives it in its own browser on its own host, and exposes it as
//! `POST /reauth` on its worker API. Until now the only callers of that
//! surface were the renewal loop and a rendered host helper; an operator
//! reading `brama subscriptions list` had no command that closes the loop the
//! runbook describes -- "sign the subscription in again, then refresh".
//! This module is that command.
//!
//! Nothing here opens a browser and nothing here reads or prints credential
//! material. The one secret involved -- Weles's worker API token -- is read
//! from the worker's own environment files exactly the way Weles's launcher
//! reads them, is sent only as a request header, and reaches neither argv nor
//! the journal. What the sign-in mints is written by Weles into the vault; the
//! proof this command reports is the same proof the runbook names: the refresh
//! that follows answers `refreshed`.

use std::time::Duration;

use serde_json::{json, Value};

use crate::subscription_dispatch::pool;

/// A run whose sign-in was confirmed and whose follow-up refresh obtained a
/// credential, and every other run. The caller's exit status is read off this,
/// so the two words are fixed here rather than spelled at each return.
const SIGNED_IN: &str = "signed_in";
const FAILED: &str = "failed";

/// The selector Weles's health answer must advertise before a named account is
/// asked for. A release without it would silently pick a sign-in row of its
/// own, and the cost of finding that out afterwards is one real sign-in into
/// the wrong account.
const LOGIN_ITEM_SELECTOR: &str = "login_item";

/// Where the worker's own API token lives, in the order Weles's launcher
/// sources them: the later file wins. Naming a single file once made the host
/// helper report "no worker API token" on a host that had one, so this reads
/// the same set in the same order the helper settled on.
const WORKER_ENV_FILES: &[&str] = &[
    "weles/var/worker-content.env",
    ".config/weles/worker.env",
    ".weles/secrets.env",
    ".stado/weles-model.env",
];

/// What one sign-in was asked to do.
pub struct SignInOptions {
    /// The provider whose account should be signed in (`codex`, `claude-code`,
    /// `kimi`).
    pub provider: String,
    /// The exact Weles sign-in row to drive, or `None` to use the single row
    /// Weles holds for the provider. Two or more rows are never guessed
    /// between.
    pub login_item: Option<String>,
    /// Why this sign-in is being run; recorded in the journal beside the
    /// verdict.
    pub reason: String,
    /// How long Weles may spend driving the browser, in milliseconds. A
    /// sign-in walks a real SSO and a consent screen, so the budget is
    /// minutes, not seconds.
    pub login_timeout_ms: u64,
}

/// Sign one provider account in through Weles, then prove the repair the way
/// the runbook does: by a refresh that answers `refreshed`.
///
/// Hard refusals -- an unknown provider, a missing reason, no reachable Weles
/// worker -- are `Err` and print as one sentence. A run that reached Weles
/// reports a verdict whatever happened, so the journal records what the
/// product answered even when the answer is a refusal.
pub async fn sign_in_provider(options: SignInOptions) -> Result<Value, String> {
    let provider = options.provider.trim().to_string();
    let weles_provider = match provider.as_str() {
        "claude-code" => "claude",
        "codex" => "codex",
        "kimi" => "kimi",
        "" => return Err("a provider is required".into()),
        other => {
            return Err(format!(
                "Weles signs in claude-code, codex and kimi; `{other}` is not one of them"
            ))
        }
    };
    let reason = options.reason.trim().to_string();
    if reason.is_empty() {
        return Err("--reason must say why this sign-in is being run".into());
    }

    let base = worker_api_base();
    let token = worker_api_token()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(transport_timeout_seconds()))
        .build()
        .map_err(|error| format!("cannot build an HTTP client: {error}"))?;

    // Everything that can be known before a browser opens is checked here,
    // because the cost of finding out afterwards is one real sign-in into the
    // wrong account.
    let health = read_health(&client, &base).await?;
    let login_item = resolve_login_item(
        &health,
        weles_provider,
        options.login_item.as_deref().map(str::trim),
    )?;

    let body = json!({
        "provider": weles_provider,
        "login_item": login_item,
        "timeout_ms": options.login_timeout_ms,
    });
    let response = client
        .post(format!("{base}/reauth"))
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("Weles worker API refused the sign-in request: {error}"))?;
    let status = response.status().as_u16();
    let answer: Value = response.json().await.unwrap_or_else(|_| json!({}));

    // A login that reports success proves nothing on its own. Confirmation is
    // Weles echoing back the exact row it was asked for; a release that
    // predates the selector answers no `login_item` at all, and that run is
    // reported unconfirmed rather than attributed to an account nobody proved
    // it came from.
    let echoed = answer.get(LOGIN_ITEM_SELECTOR).and_then(Value::as_str);
    let confirmed = status == 200 && echoed == Some(login_item.as_str());
    let account = answer
        .get("display_name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    if !confirmed {
        let detail = format!(
            "Weles answered HTTP {status} and named `{}` where `{login_item}` was asked for, so \
             this run is not attributed to the account it was meant for; the credential it may \
             have minted stands in the vault, and `brama subscriptions list` says whether the \
             pool recovered",
            echoed.unwrap_or("no login_item")
        );
        return Ok(verdict(
            &provider,
            &login_item,
            &reason,
            FAILED,
            status,
            &account,
            detail,
            Value::Null,
        ));
    }

    // The runbook's own proof: a sign-in replaced the grant, and the refresh
    // that follows answers `refreshed`. The refresh writes its own journal
    // record beside this one, exactly as if the operator had run it.
    let refresh = pool::refresh_provider(&provider, &reason).await?;
    let refreshed = refresh.get("result").and_then(Value::as_str) == Some("refreshed");
    let detail = if refreshed {
        format!("Weles signed `{login_item}` in and the refresh that followed obtained a credential")
    } else {
        format!(
            "Weles signed `{login_item}` in, but the refresh that followed obtained nothing: {}",
            refresh
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or("no detail was reported")
        )
    };
    Ok(verdict(
        &provider,
        &login_item,
        &reason,
        if refreshed { SIGNED_IN } else { FAILED },
        status,
        &account,
        detail,
        refresh,
    ))
}

/// One verdict, in the shape the caller prints and the audit record keeps.
/// Both are written here so a record cannot say something the operator was
/// never told.
#[allow(clippy::too_many_arguments)]
fn verdict(
    provider: &str,
    login_item: &str,
    reason: &str,
    result: &str,
    http_status: u16,
    account: &str,
    detail: String,
    refresh: Value,
) -> Value {
    crate::journal::record_subscription_sign_in(provider, login_item, reason, result, &detail);
    json!({
        "provider": provider,
        "login_item": login_item,
        "account": account,
        "result": result,
        "http_status": http_status,
        "detail": detail,
        "refresh": refresh,
    })
}

/// Where Weles's worker API answers. It binds loopback on its own host, so a
/// deployment where Weles runs elsewhere names that host explicitly.
fn worker_api_base() -> String {
    let host = env_or("WELES_API_HOST", "127.0.0.1");
    let port = env_or("WELES_API_PORT", "8788");
    format!("http://{host}:{port}")
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
fn transport_timeout_seconds() -> u64 {
    env_or("BRAMA_SIGN_IN_TRANSPORT_TIMEOUT_SECONDS", "1200")
        .parse()
        .unwrap_or(1200)
}

/// The worker API token, read the way Weles's own launcher reads it and never
/// echoed anywhere. `WELES_API_TOKEN` in the environment wins; otherwise the
/// worker's environment files are read in launcher order and the later file
/// wins, accepting `WELES_CONSOLE_API_TOKEN` as the console-minted alias.
fn worker_api_token() -> Result<String, String> {
    if let Ok(token) = std::env::var("WELES_API_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let files: Vec<std::path::PathBuf> = match std::env::var("WELES_WORKER_ENV_FILE") {
        Ok(file) if !file.trim().is_empty() => vec![std::path::PathBuf::from(file.trim())],
        _ => WORKER_ENV_FILES
            .iter()
            .map(|relative| std::path::PathBuf::from(&home).join(relative))
            .collect(),
    };
    let mut token = None;
    let mut present = false;
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        present = true;
        for key in ["WELES_API_TOKEN", "WELES_CONSOLE_API_TOKEN"] {
            if let Some(found) = env_file_value(&text, key) {
                token = Some(found);
            }
        }
    }
    if !present {
        return Err(
            "no Weles worker environment file exists on this host, so it has no worker API \
             token; sign-ins run on the host Weles runs on"
                .into(),
        );
    }
    token.ok_or_else(|| {
        format!(
            "no WELES_API_TOKEN or WELES_CONSOLE_API_TOKEN in any of: {}",
            files
                .iter()
                .map(|file| file.display().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        )
    })
}

/// The last assignment of `key` in one env file, `export` prefix and quotes
/// tolerated, exactly as the shell that sources the file would read it.
fn env_file_value(text: &str, key: &str) -> Option<String> {
    let mut value = None;
    for line in text.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        if let Some(rest) = line.strip_prefix(key) {
            if let Some(assigned) = rest.strip_prefix('=') {
                let assigned = assigned.trim().trim_matches('"').to_string();
                if !assigned.is_empty() {
                    value = Some(assigned);
                }
            }
        }
    }
    value
}

/// Weles's own health answer, which advertises the selector contract and the
/// sign-in rows it holds. `error_for_status` matters: a healthy exit from an
/// unrelated service that happens to hold this port must not send a sign-in
/// request nobody serves.
async fn read_health(client: &reqwest::Client, base: &str) -> Result<Value, String> {
    let response = client
        .get(format!("{base}/healthz"))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| {
            format!(
                "Weles worker API does not answer its own health check at {base}/healthz \
                 ({error}); start it before signing an account in"
            )
        })?;
    response
        .json()
        .await
        .map_err(|error| format!("Weles health answer is not JSON: {error}"))
}

/// The exact sign-in row this run will drive.
///
/// A named row must exist and belong to the provider. An unnamed run uses the
/// single row Weles holds for the provider; zero rows cannot be signed in and
/// two or more are never guessed between, because a sign-in there would not
/// say which account it was for.
fn resolve_login_item(
    health: &Value,
    weles_provider: &str,
    asked: Option<&str>,
) -> Result<String, String> {
    let features = health.get("features").and_then(Value::as_array);
    let advertised = features.is_some_and(|features| {
        features
            .iter()
            .any(|feature| feature.as_str() == Some(LOGIN_ITEM_SELECTOR))
    });
    if !advertised {
        return Err(format!(
            "this Weles release does not advertise the {LOGIN_ITEM_SELECTOR} selector, so it \
             would choose a sign-in row itself; deploy the release that carries it before \
             signing a named account in"
        ));
    }
    let rows: Vec<&Value> = health
        .get("login_items")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().collect())
        .unwrap_or_default();
    let row_item = |row: &&Value| {
        row.get(LOGIN_ITEM_SELECTOR)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    if let Some(asked) = asked.filter(|asked| !asked.is_empty()) {
        let named: Vec<&&Value> = rows.iter().filter(|row| row_item(row) == asked).collect();
        if named.is_empty() {
            let held = rows
                .iter()
                .map(row_item)
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "Weles holds no sign-in row for {asked}; it holds {}. That account has to exist \
                 in Weles before it can be signed in",
                if held.is_empty() { "none".into() } else { held }
            ));
        }
        let providers: Vec<&str> = named
            .iter()
            .filter_map(|row| row.get("provider").and_then(Value::as_str))
            .collect();
        if !providers.is_empty() && providers.iter().all(|held| *held != weles_provider) {
            return Err(format!(
                "{asked} is a {} account, not a {weles_provider} one; refusing to sign it in \
                 for the wrong provider",
                providers[0]
            ));
        }
        return Ok(asked.to_string());
    }
    let held: Vec<String> = rows
        .iter()
        .filter(|row| row.get("provider").and_then(Value::as_str) == Some(weles_provider))
        .map(row_item)
        .filter(|item| !item.is_empty())
        .collect();
    match held.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(format!(
            "Weles holds no sign-in row for provider {weles_provider}; that account has to \
             exist in Weles before it can be signed in"
        )),
        several => Err(format!(
            "Weles holds {} sign-in rows for provider {weles_provider} ({}); name the one to \
             drive with --login-item",
            several.len(),
            several.join(", ")
        )),
    }
}
