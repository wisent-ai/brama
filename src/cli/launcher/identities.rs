//! The bearers and request-sign secrets the gateway preloads at start, read
//! from their exact Skarbiec items through the entitlements router. Each
//! router call loads the whole vault, so the reads run concurrently.

use std::path::Path;
use std::process::Command;

use serde_json::{json, Map, Value};

/// The v2 item schema the router answers with.
const ITEM_SCHEMA: &str = "skarbiec.item.v2";

/// One preloaded model-router client: its id, the vault item holding its
/// bearer, the agent it signs as, its alias allowlist, and whether the
/// gateway refuses to start without it.
struct Client {
    id: &'static str,
    item: &'static str,
    agent: Option<&'static str>,
    models: Option<Vec<String>>,
    required: bool,
}

/// The fields of one vault item, or why the router would not give them.
fn item_fields(router: &Path, item: &str) -> Result<Map<String, Value>, String> {
    let output = Command::new(router)
        .args(["get", item])
        .output()
        .map_err(|error| format!("reading {item} through the entitlements router failed: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() { String::from_utf8_lossy(&output.stdout).trim().to_string() } else { stderr.trim().to_string() };
        return Err(format!("reading {item} through the entitlements router failed: {detail}"));
    }
    let payload: Value = serde_json::from_slice(&output.stdout).map_err(|error| format!("{item}: {error}"))?;
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

/// One field of one item, printed bare.
pub(super) fn item_field(router: &Path, item: &str, name: &str) -> Result<String, String> {
    field(&item_fields(router, item)?, item, name)
}

/// The model-router client table, as JSON.
pub(super) fn model_router(router: &Path, allowed: &str, backend_models: &str) -> Result<String, String> {
    let backend: Vec<String> = serde_json::from_str(backend_models).map_err(|error| format!("backend models: {error}"))?;
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
        Client { id: "weles", item: "weles-model-router", agent: Some("weles"), models: Some(vec!["best".into(), "weles".into()]), required: true },
        Client { id: "wisent-backend", item: "wisent-backend-model-router", agent: Some("wisent-app"), models: Some(backend), required: true },
        Client { id: "wisent-app", item: "wisent-app-model-router", agent: Some("wisent-app"), models: Some(renewal), required: false },
        Client { id: "brama-desktop", item: "brama-desktop-model-router", agent: None, models: None, required: false },
    ];
    let answers: Vec<Result<String, String>> = std::thread::scope(|scope| {
        let reads: Vec<_> = clients
            .iter()
            .map(|client| scope.spawn(move || item_fields(router, client.item).and_then(|fields| {
                fields
                    .get("token")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty() && value.trim() == *value)
                    .map(str::to_string)
                    .ok_or_else(|| format!("{}/token is not a single non-empty value", client.item))
            })))
            .collect();
        reads.into_iter().map(|read| read.join().unwrap_or_else(|_| Err("a router read panicked".into()))).collect()
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
    let sources = [
        ("echo", "echo-agent-auth"),
        ("content-platform", "content-platform-agent-auth"),
        ("oko", "oko-model-agent-auth"),
        ("weles", "weles-model-agent-auth"),
        ("lem", "lem-agent-auth"),
        ("probierz", "probierz-agent-auth"),
        ("wisent-app", "agent:wisent-app"),
    ];
    let answers: Vec<Result<(String, String), String>> = std::thread::scope(|scope| {
        let reads: Vec<_> = sources
            .iter()
            .map(|&(expected, item)| scope.spawn(move || {
                let fields = item_fields(router, item)?;
                // `wisent-app` is Jeden's public runtime identity, held in the
                // dedicated `agent:wisent-app` item.
                if expected == "wisent-app" {
                    return Ok((expected.to_string(), field(&fields, item, "value")?));
                }
                if field(&fields, item, "id")? != expected {
                    return Err(format!("{item}/id does not match its product identity"));
                }
                Ok((expected.to_string(), field(&fields, item, "agent_auth_secret")?))
            }))
            .collect();
        reads.into_iter().map(|read| read.join().unwrap_or_else(|_| Err("a router read panicked".into()))).collect()
    });
    let identities: Map<String, Value> = answers
        .into_iter()
        .map(|answer| answer.map(|(id, secret)| (id, Value::String(secret))))
        .collect::<Result<_, _>>()?;
    Ok(serde_json::to_string(&identities).expect("identities serialise"))
}
