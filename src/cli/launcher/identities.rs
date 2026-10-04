//! The bearers and request-sign secrets the gateway preloads at start, read
//! through the entitlements router from the items that play their roles. No
//! item id is written here: one `list` says which live item carries
//! `stado:role:<role>` for every role, and only then is each item read by the
//! id the vault gave it, so renaming or replacing an item changes nothing.
//! Each router call loads the whole vault, so the reads run concurrently.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde_json::{json, Map, Value};

/// The v2 item schema the router answers with.
const ITEM_SCHEMA: &str = "skarbiec.item.v2";

/// The tag an item carries to play a role.
const ROLE_TAG: &str = "stado:role:";

/// One preloaded model-router client: its id, the role whose item holds its
/// bearer, the agent it signs as, its alias allowlist, and whether the
/// gateway refuses to start without it.
struct Client {
    id: &'static str,
    role: &'static str,
    agent: Option<&'static str>,
    models: Option<Vec<String>>,
    required: bool,
}

/// One row of the router's bare `list`.
#[derive(serde::Deserialize)]
struct Listed {
    #[serde(default)]
    id: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    deleted: bool,
}

/// Every live item id carrying each role tag, from one listing. `envs` is the
/// environment the router runs with: empty inside the launcher, which already
/// carries it, and service.env's settings for a caller outside it.
fn role_holders(
    router: &Path,
    envs: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, Vec<String>>, String> {
    let output = Command::new(router)
        .arg("list")
        .envs(envs)
        .output()
        .map_err(|error| {
            format!("listing the vault through the entitlements router failed: {error}")
        })?;
    if !output.status.success() {
        return Err(format!(
            "listing the vault through the entitlements router failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let rows: Vec<Listed> = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("the vault listing is not a list of items: {error}"))?;
    let mut holders: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for row in rows.into_iter().filter(|row| !row.deleted) {
        for role in row.tags.iter().filter_map(|tag| tag.strip_prefix(ROLE_TAG)) {
            holders
                .entry(role.to_string())
                .or_default()
                .push(row.id.clone());
        }
    }
    Ok(holders)
}

/// The one item playing `role`. No item in the role, or several, is refused
/// with the role and the tag that selects it.
fn item_for_role<'a>(
    holders: &'a BTreeMap<String, Vec<String>>,
    role: &str,
) -> Result<&'a str, String> {
    match holders.get(role).map(Vec::as_slice) {
        Some([one]) => Ok(one.as_str()),
        None | Some([]) => Err(format!(
            "no vault item carries {ROLE_TAG}{role}; tag the item that plays role {role} with it"
        )),
        Some(several) => Err(format!(
            "{} vault items carry {ROLE_TAG}{role}; exactly one item may play role {role}",
            several.len()
        )),
    }
}

/// The fields of one vault item, or why the router would not give them.
fn item_fields(
    router: &Path,
    envs: &BTreeMap<String, String>,
    item: &str,
) -> Result<Map<String, Value>, String> {
    let output = Command::new(router)
        .args(["get", item])
        .envs(envs)
        .output()
        .map_err(|error| {
            format!("reading {item} through the entitlements router failed: {error}")
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(format!(
            "reading {item} through the entitlements router failed: {detail}"
        ));
    }
    let payload: Value =
        serde_json::from_slice(&output.stdout).map_err(|error| format!("{item}: {error}"))?;
    if payload.get("schema").and_then(Value::as_str) != Some(ITEM_SCHEMA) {
        return Err(format!("{item} did not return a Skarbiec v2 item"));
    }
    payload
        .get("fields")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| format!("{item} did not return a fields object"))
}

fn field(fields: &Map<String, Value>, item: &str, name: &str) -> Result<String, String> {
    fields
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{item}/{name} is empty"))
}

/// One field of the item that plays `role`, printed bare.
pub(super) fn role_field(router: &Path, role: &str, name: &str) -> Result<String, String> {
    role_field_in(router, &BTreeMap::new(), role, name)
}

/// One field of the item that plays `role`, read with the router running in
/// `envs` — what a command outside the launcher, such as `brama probe`, passes
/// from service.env so the router finds the vault.
pub(crate) fn role_field_in(
    router: &Path,
    envs: &BTreeMap<String, String>,
    role: &str,
    name: &str,
) -> Result<String, String> {
    let holders = role_holders(router, envs)?;
    let item = item_for_role(&holders, role)?;
    field(&item_fields(router, envs, item)?, role, name)
}

/// The model-router client table, as JSON.
pub(super) fn model_router(
    router: &Path,
    allowed: &str,
    backend_models: &str,
) -> Result<String, String> {
    let backend: Vec<String> =
        serde_json::from_str(backend_models).map_err(|error| format!("backend models: {error}"))?;
    // The renewal caller's routes are derived from the same closed allowlist:
    // each provider's own route is what a reauth trajectory probes, and `best`
    // is the subscription alias those grants serve.
    let renewal_providers = ["claude-code", "codex", "kimi"];
    let mut renewal: Vec<String> = allowed
        .split(',')
        .filter(|model| renewal_providers.contains(&model.split('/').next().unwrap_or("")))
        .map(str::to_string)
        .chain(["best".to_string()])
        .collect();
    renewal.sort();
    renewal.dedup();
    let clients = [
        Client {
            id: "weles",
            role: "weles-model-router",
            agent: Some("weles"),
            models: Some(vec!["best".into(), "weles".into()]),
            required: true,
        },
        Client {
            id: "wisent-backend",
            role: "wisent-backend-model-router",
            agent: Some("wisent-app"),
            models: Some(backend),
            required: true,
        },
        Client {
            id: "wisent-app",
            role: "wisent-app-model-router",
            agent: Some("wisent-app"),
            models: Some(renewal),
            required: false,
        },
        Client {
            id: "brama-desktop",
            role: "brama-desktop-model-router",
            agent: None,
            models: None,
            required: false,
        },
    ];
    let holders = role_holders(router, &BTreeMap::new())?;
    let answers: Vec<Result<String, String>> = std::thread::scope(|scope| {
        let reads: Vec<_> = clients
            .iter()
            .map(|client| {
                let holders = &holders;
                scope.spawn(move || {
                    let item = item_for_role(holders, client.role)?;
                    item_fields(router, &BTreeMap::new(), item).and_then(|fields| {
                        fields
                            .get("token")
                            .and_then(Value::as_str)
                            .filter(|value| !value.is_empty() && value.trim() == *value)
                            .map(str::to_string)
                            .ok_or_else(|| {
                                format!(
                                    "the token of role {} is not a single non-empty value",
                                    client.role
                                )
                            })
                    })
                })
            })
            .collect();
        reads
            .into_iter()
            .map(|read| {
                read.join()
                    .unwrap_or_else(|_| Err("a router read panicked".into()))
            })
            .collect()
    });
    let mut identities = Vec::new();
    for (client, answer) in clients.iter().zip(answers) {
        let token = match answer {
            Ok(token) => token,
            Err(detail) if client.required => return Err(detail),
            Err(detail) => {
                eprintln!("optional client {} skipped: {detail}", client.id);
                continue;
            }
        };
        let mut entry = json!({ "client_id": client.id, "token": token });
        if let Some(agent) = client.agent {
            entry["agent_id"] = json!(agent);
        }
        if let Some(models) = &client.models {
            entry["allowed_models"] = json!(models);
        }
        identities.push(entry);
    }
    Ok(serde_json::to_string(&identities).expect("identities serialise"))
}

/// The request-sign identities of every product, as a JSON object.
pub(super) fn request_sign(router: &Path) -> Result<String, String> {
    // Each product's request-sign secret plays the role `<product>-agent-auth`;
    // `wisent-app` is Jeden's public runtime identity, whose item plays
    // `wisent-app-agent` and holds it under `value`.
    let sources = [
        ("echo", "echo-agent-auth"),
        ("content-platform", "content-platform-agent-auth"),
        ("oko", "oko-agent-auth"),
        ("weles", "weles-agent-auth"),
        ("lem", "lem-agent-auth"),
        ("probierz", "probierz-agent-auth"),
        ("wisent-app", "wisent-app-agent"),
    ];
    let holders = role_holders(router, &BTreeMap::new())?;
    let answers: Vec<Result<(String, String), String>> = std::thread::scope(|scope| {
        let reads: Vec<_> = sources
            .iter()
            .map(|&(expected, role)| {
                let holders = &holders;
                scope.spawn(move || {
                    let item = item_for_role(holders, role)?;
                    let fields = item_fields(router, &BTreeMap::new(), item)?;
                    if expected == "wisent-app" {
                        return Ok((expected.to_string(), field(&fields, role, "value")?));
                    }
                    if field(&fields, role, "id")? != expected {
                        return Err(format!(
                            "role {role}: its id does not match its product identity"
                        ));
                    }
                    Ok((
                        expected.to_string(),
                        field(&fields, role, "agent_auth_secret")?,
                    ))
                })
            })
            .collect();
        reads
            .into_iter()
            .map(|read| {
                read.join()
                    .unwrap_or_else(|_| Err("a router read panicked".into()))
            })
            .collect()
    });
    let identities: Map<String, Value> = answers
        .into_iter()
        .map(|answer| answer.map(|(id, secret)| (id, Value::String(secret))))
        .collect::<Result<_, _>>()?;
    Ok(serde_json::to_string(&identities).expect("identities serialise"))
}
