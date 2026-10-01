//! `brama probe`: ask the running gateway on this host to serve one real
//! request per alias and report what came back.
//!
//! `/health` answering says the process started; it says nothing about
//! whether a provider credential can be redeemed, which is the failure this
//! command exists for: healthy and serving nothing. Each alias is a real
//! completion through the gateway's own listener, so a line with a body means
//! a model answered. It makes requests and changes nothing.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use clap::Args;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::workload::{home, service_settings};

#[derive(Args)]
pub(crate) struct ProbeArgs {
    /// Alias to request; repeat for each. `best` is sent with the signed
    /// agent identity the subscription route requires.
    #[arg(long = "alias", default_values_t = ["best".to_string(), "local-openai/chat-primary".to_string(), "wisent-backend".to_string()])]
    aliases: Vec<String>,
}

/// The loopback port the gateway listens on when service.env names none.
const DEFAULT_PORT: &str = "8080";
/// What the gateway logs when it binds, followed by the address.
const ANNOUNCEMENT: &str = "Starting brama server on ";
/// The vault item holding the bearer the probe presents.
const BEARER_ITEM: &str = "echo-model-router";
/// The agent whose request-sign secret signs a `best` request.
const SIGNING_AGENT: &str = "echo";
const SIGNING_ITEM: &str = "echo-agent-auth";

/// One field of one vault item, read through the router with the
/// environment the launcher builds from service.env: without it the router
/// reports "vault not initialized" about a vault that is fine.
fn vault_field(
    router: &str,
    settings: &BTreeMap<String, String>,
    item: &str,
    field: &str,
) -> Result<String, String> {
    let output = Command::new(router)
        .args(["get", item])
        .envs(settings)
        .output()
        .map_err(|error| format!("{router}: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr)
            .trim()
            .replace('\n', " "));
    }
    let payload: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "the router did not return a Skarbiec item".to_string())?;
    payload
        .pointer(&format!("/fields/{field}"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{item}/{field} is empty"))
}

/// Loopback first, then the address the last boot announced; plain HTTP is
/// refused off loopback and the log may name an older bind, so neither alone
/// is reliable.
fn candidates(settings: &BTreeMap<String, String>, home: &Path) -> Vec<String> {
    let port = settings
        .get("PORT")
        .cloned()
        .unwrap_or_else(|| DEFAULT_PORT.to_string());
    let mut candidates = vec![format!("127.0.0.1:{port}")];
    let log = std::fs::read(
        home.join(".stado/logs")
            .join(format!("{}.log", super::diagnose::SERVICE_LABEL)),
    )
    .unwrap_or_default();
    let announced = String::from_utf8_lossy(&log)
        .lines()
        .filter_map(|line| {
            line.split_once(ANNOUNCEMENT)
                .map(|(_, rest)| rest.trim().to_string())
        })
        .last();
    if let Some(announced) = announced.filter(|address| !candidates.contains(address)) {
        candidates.push(announced);
    }
    candidates
}

async fn probe(args: ProbeArgs) -> Result<(), String> {
    let home = home();
    let settings = service_settings(&home)?;
    let router = settings
        .get("ENTITLEMENTS_ROUTER_BIN")
        .cloned()
        .ok_or("service env names no entitlements router")?;
    let token = vault_field(&router, &settings, BEARER_ITEM, "token")
        .map_err(|error| format!("cannot read a bearer from the router: {error}"))?;
    let client = reqwest::Client::new();
    let mut base = None;
    for authority in candidates(&settings, &home) {
        match client
            .get(format!("http://{authority}/health"))
            .send()
            .await
        {
            Ok(answer) => {
                println!("health {} at {authority}", answer.status().as_u16());
                base = Some(format!("http://{authority}"));
                break;
            }
            Err(error) => println!("{authority}: {error}"),
        }
    }
    let base = base.ok_or("no candidate address served /health")?;
    // The secret stays in this process: never printed, never in argv.
    let secret = vault_field(&router, &settings, SIGNING_ITEM, "agent_auth_secret")
        .inspect_err(|problem| println!("request signing unavailable: {problem}"))
        .ok();
    for alias in args.aliases {
        let body = serde_json::to_vec(
            &json!({ "model": alias, "messages": [{ "role": "user", "content": "say ok" }] }),
        )
        .expect("payload serialises");
        let mut request = client
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth(&token)
            .header("Content-Type", "application/json");
        if let (true, Some(secret)) = (alias == "best", &secret) {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or_default()
                .to_string();
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                .map_err(|error| error.to_string())?;
            mac.update(
                format!(
                    "{SIGNING_AGENT}:{stamp}:{}",
                    hex::encode(Sha256::digest(&body))
                )
                .as_bytes(),
            );
            request = request
                .header("x-agent-id", SIGNING_AGENT)
                .header("x-agent-timestamp", stamp)
                .header(
                    "x-agent-signature",
                    hex::encode(mac.finalize().into_bytes()),
                );
        }
        match request.body(body).send().await {
            Ok(answer) => {
                let status = answer.status();
                let text = answer.text().await.unwrap_or_default();
                if status.is_success() {
                    let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                    let said = payload
                        .pointer("/choices/0/message/content")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let served = payload.get("model").and_then(Value::as_str).unwrap_or("");
                    println!("{alias} {} model={served} said={said:?}", status.as_u16());
                } else {
                    println!(
                        "{alias} {} {}",
                        status.as_u16(),
                        text.trim().replace('\n', " ")
                    );
                }
            }
            Err(error) => println!("{alias} unreachable {error}"),
        }
    }
    Ok(())
}

pub(crate) async fn run(args: ProbeArgs) {
    if let Err(detail) = probe(args).await {
        eprintln!("{detail}");
        std::process::exit(1);
    }
}
