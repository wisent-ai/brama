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

use serde_json::Value;

/// Where Weles is, and what Stado knows about that placement.
///
/// A forwarded loopback URL does not identify the host serving Weles.
/// Retain Stado's placement and observation so failures name that host and
/// expose the freshness of the route.
pub(crate) struct WelesEndpoint {
    pub url: String,
    /// The host Stado places the service on, when its answer names one.
    pub placed_on: Option<String>,
    /// How fresh Stado's observation of it is, in Stado's own words.
    pub observed: Option<String>,
}

impl WelesEndpoint {
    /// What to say about this endpoint when a call to it fails: the host
    /// behind the forward, how stale the placement reading is, and the read
    /// that shows that host's own log.
    pub fn whereabouts(&self) -> String {
        let Some(host) = self.placed_on.as_deref().filter(|host| !host.is_empty()) else {
            return format!(
                "{} is a local address and Stado's service directory names no host behind it; \
                 `stado service directory connect weles-admission --consumer operator --json` \
                 says what it does know",
                self.url
            );
        };
        let freshness = self
            .observed
            .as_deref()
            .filter(|observed| !observed.is_empty())
            .map(|observed| format!(", last observed {observed}"))
            .unwrap_or_default();
        format!(
            "{} is a forward to weles-api on {host}{freshness}; read that service's own log with \
             `stado service logs weles-api --host {host}`",
            self.url
        )
    }
}

/// Resolve Weles from Stado at the moment a sign-in needs it. Placement can
/// change while Brama keeps serving model traffic; baking loopback into the
/// launcher made the renewal path silently keep the old host forever.
pub(crate) async fn worker_api_base() -> Result<WelesEndpoint, String> {
    if let Ok(configured) = std::env::var("BRAMA_WELES_URL") {
        let configured = configured.trim();
        if !configured.is_empty() {
            reqwest::Url::parse(configured)
                .map_err(|error| format!("BRAMA_WELES_URL is invalid: {error}"))?;
            return Ok(WelesEndpoint {
                url: configured.trim_end_matches('/').to_string(),
                placed_on: None,
                observed: None,
            });
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
    // A host may declare one resolver adapter per consumer, and Stado refuses
    // to guess between them: without a named consumer `brama subscription
    // sign-in` dies on "<host> declares 3 resolver adapters for
    // weles-admission, one per consumer (...); name the caller with
    // --consumer", so no account can be signed in from the command line at
    // all. The sibling lookup in `cli::subscriptions::sync` already names its
    // consumer; this one did not.
    let consumer = env_or("BRAMA_WELES_ADMISSION_CONSUMER", "operator");
    let output = tokio::process::Command::new(&stado)
        .kill_on_drop(true)
        .args([
            "service",
            "directory",
            "connect",
            "weles-admission",
            "--consumer",
            consumer.as_str(),
            "--no-verify",
            "--json",
        ])
        .output()
        .await
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
    let text = |field: &str| {
        document
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    Ok(WelesEndpoint {
        url: url.trim_end_matches('/').to_string(),
        placed_on: text("placed_on"),
        observed: text("observed"),
    })
}

fn env_or(key: &str, default: &str) -> String {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => default.to_string(),
    }
}

/// Brama's Weles admission credential. It is deliberately distinct from
/// Weles's general worker API token.
///
/// The launcher exports it at every service start, read from
/// `brama-weles-reauth` through the vault. An operator running
/// `brama subscription sign-in` has no launcher, and until this read existed
/// that command answered `BRAMA_WELES_REAUTH_TOKEN is unavailable` and
/// stopped — the one repair the gateway itself names for a grant the provider
/// will not refresh again (`invalid_grant`) could be performed only by the
/// service, never by the person holding the refusal. The same item, read the
/// same way, through the same vault program every other credential operation
/// runs.
pub(crate) fn worker_api_token() -> Result<String, String> {
    let declared = std::env::var("BRAMA_WELES_REAUTH_TOKEN")
        .unwrap_or_default()
        .trim()
        .to_string();
    if !declared.is_empty() {
        return Ok(declared);
    }
    vault_reauth_token()
}

/// The role the Weles sign-in worker's bearer plays in the vault. The one
/// live item carrying `stado:role:<role>` holds it; no item id is named, so
/// renaming or replacing the item changes nothing here.
const REAUTH_ROLE: &str = "brama-weles-reauth";

/// One row of the vault's bare `list`.
#[derive(serde::Deserialize)]
struct ListedItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    deleted: bool,
}

/// The id of the one live item that plays [`REAUTH_ROLE`]. No item in the
/// role, or several, is refused with the role, so a vault whose item lost its
/// tag is told apart from one holding two.
fn reauth_item(program: &str) -> Result<String, String> {
    let tag = format!("stado:role:{REAUTH_ROLE}");
    let output = std::process::Command::new(program)
        .arg("list")
        .output()
        .map_err(|error| {
            format!(
                "cannot list the vault to find role {REAUTH_ROLE}: {error} (running {program}; \
                 declare another with ENTITLEMENTS_ROUTER_BIN or SKARBIEC_BIN, or export \
                 BRAMA_WELES_REAUTH_TOKEN)"
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "cannot list the vault to find role {REAUTH_ROLE}: {program} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let items: Vec<ListedItem> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("the vault listing is not a list of items: {error}"))?;
    let holders: Vec<String> = items
        .into_iter()
        .filter(|item| !item.deleted && item.tags.iter().any(|each| each == &tag))
        .map(|item| item.id)
        .collect();
    match holders.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(format!(
            "no vault item carries {tag}; tag the item that holds the Weles sign-in worker's \
             bearer with it, or export BRAMA_WELES_REAUTH_TOKEN"
        )),
        several => Err(format!(
            "{} vault items carry {tag}; exactly one item may play role {REAUTH_ROLE}",
            several.len()
        )),
    }
}

/// The `token` field of the item that plays [`REAUTH_ROLE`], read from the
/// vault this machine carries.
///
/// Blocking rather than the gateway's bounded async read, because this runs
/// once per sign-in, before any HTTP exchange, and both callers are already
/// waiting on a child process for the length of a browser login.
fn vault_reauth_token() -> Result<String, String> {
    let program = crate::gateway::broker::entitlements_router_bin();
    let item = reauth_item(&program)?;
    let output = std::process::Command::new(&program)
        .args(["get", item.as_str()])
        .output()
        .map_err(|error| format!("cannot read role {REAUTH_ROLE} ({item}): {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cannot read role {REAUTH_ROLE} ({item}): {program} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let payload: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!("role {REAUTH_ROLE} ({item}) did not answer a Skarbiec item: {error}")
    })?;
    if payload.get("schema").and_then(serde_json::Value::as_str) != Some("skarbiec.item.v2") {
        return Err(format!(
            "role {REAUTH_ROLE} ({item}) did not return a Skarbiec v2 item"
        ));
    }
    let token = payload
        .get("fields")
        .and_then(|fields| fields.get("token"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(format!("role {REAUTH_ROLE} ({item}) has an empty token"));
    }
    Ok(token)
}
