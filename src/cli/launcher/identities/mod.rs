//! The bearers and request-sign secrets the gateway preloads at start, read
//! through the entitlements router from the items that play their roles. No
//! item id is written here: one `list` says which live item carries
//! `stado:role:<role>` for every role, and only then is each item read by the
//! id the vault gave it, so renaming or replacing an item changes nothing.
//! Each router call loads the whole vault, so the reads run concurrently.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Map, Value};

mod vault;
pub(super) use vault::role_field;
pub(crate) use vault::role_field_in;
use vault::{field, item_fields, item_for_role, role_holders};

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

/// The model-router client table, as JSON.
pub(super) fn model_router(
    router: &Path,
    allowed: &str,
    backend_models: &str,
) -> Result<String, String> {
    let backend: Vec<String> =
        serde_json::from_str(backend_models).map_err(|error| format!("backend models: {error}"))?;
    // The renewal caller's routes are derived from the same closed allowlist:
    // each route whose provider is one of the pool's subscription providers
    // is what a reauth trajectory probes, and `best` is the subscription
    // alias those grants serve.
    let mut renewal: Vec<String> = allowed
        .split(',')
        .filter(|model| {
            brama::subscription_dispatch::dispatch::provider_requires_caller_identity(model)
        })
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
        reads.into_iter().map(|read| joined(read.join())).collect()
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
    serde_json::to_string(&identities)
        .map_err(|error| format!("the model-router client table cannot be written: {error}"))
}

/// What one router read answered, or what it panicked with: a panic is a
/// refusal naming its message, not a bare "panicked".
fn joined<T>(answer: std::thread::Result<Result<T, String>>) -> Result<T, String> {
    match answer {
        Ok(answer) => answer,
        Err(payload) => Err(match payload.downcast_ref::<&str>() {
            Some(message) => format!("a router read panicked: {message}"),
            None => match payload.downcast_ref::<String>() {
                Some(message) => format!("a router read panicked: {message}"),
                None => "a router read panicked with a payload that is not text".to_string(),
            },
        }),
    }
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
        reads.into_iter().map(|read| joined(read.join())).collect()
    });
    let identities: Map<String, Value> = answers
        .into_iter()
        .map(|answer| answer.map(|(id, secret)| (id, Value::String(secret))))
        .collect::<Result<_, _>>()?;
    serde_json::to_string(&identities)
        .map_err(|error| format!("the request-sign identity table cannot be written: {error}"))
}
