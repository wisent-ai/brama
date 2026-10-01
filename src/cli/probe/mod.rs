//! `brama probe`: ask the running gateway on this host to serve one real
//! request per alias and report what came back.
//!
//! `/health` answering says the process started; it says nothing about
//! whether a provider credential can be redeemed, which is the failure this
//! command exists for: healthy and serving nothing. Each alias is a real
//! completion through the gateway's own listener, so a line with a body means
//! a model answered. It makes requests and changes nothing. The caller names
//! the client identity it probes as; no item, agent or alias is built in.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use clap::Args;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::subscriptions::unattended::split_item_field;
use super::workload::{home, service_settings};

#[derive(Args)]
pub(crate) struct ProbeArgs {
    /// Alias to request; repeat for each. Without one, every alias the
    /// gateway declares is requested.
    #[arg(long = "alias")]
    aliases: Vec<String>,
    /// `<item>#<field>` of the client bearer the requests present, read
    /// through the entitlements router service.env names
    #[arg(long, value_name = "ITEM#FIELD")]
    bearer_item: String,
    /// Agent id every request is signed as; needs --signing-item
    #[arg(long, value_name = "AGENT", requires = "signing_item")]
    agent: Option<String>,
    /// `<item>#<field>` of the request-sign secret that signs each request as
    /// --agent; needs --agent
    #[arg(long, value_name = "ITEM#FIELD", requires = "agent")]
    signing_item: Option<String>,
    /// Acknowledge that each alias spends one short provider request
    #[arg(long, default_value_t = false)]
    allow_provider_cost: bool,
    /// Print the addresses tried, request signing and every alias's answer as
    /// one JSON document instead of lines
    #[arg(long, default_value_t = false)]
    json: bool,
}

/// The loopback port the gateway listens on when service.env names none.
const DEFAULT_PORT: &str = "8080";
/// What the gateway logs when it binds, followed by the address.
const ANNOUNCEMENT: &str = "Starting brama server on ";

/// One field of one vault item, read through the router with the
/// environment the launcher builds from service.env: without it the router
/// reports "vault not initialized" about a vault that is fine.
fn vault_field(
    router: &str,
    settings: &BTreeMap<String, String>,
    coordinate: &str,
) -> Result<String, String> {
    let (item, field) = split_item_field(coordinate)?;
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
        .next_back();
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
    let token = vault_field(&router, &settings, &args.bearer_item).map_err(|error| {
        format!(
            "cannot read the bearer {} through {router}: {error}",
            args.bearer_item
        )
    })?;
    // The secret stays in this process: never printed, never in argv.
    let signing = match (&args.agent, &args.signing_item) {
        (Some(agent), Some(coordinate)) => {
            let secret = vault_field(&router, &settings, coordinate).map_err(|error| {
                format!("cannot read {coordinate} to sign as {agent}: {error}")
            })?;
            Some((agent.clone(), secret))
        }
        _ => None,
    };
    let aliases = if args.aliases.is_empty() {
        let declared = brama::core::server::alias_report()
            .map_err(|error| format!("the declared aliases could not be read: {error}"))?;
        declared
            .aliases
            .iter()
            .map(|alias| alias.alias.clone())
            .collect::<Vec<_>>()
    } else {
        args.aliases.clone()
    };
    if aliases.is_empty() {
        return Err(
            "this gateway declares no alias and none was named with --alias; \
             `brama routes set` declares one"
                .to_string(),
        );
    }
    let client = reqwest::Client::new();
    let mut report = json!({
        "addresses": [],
        "signing": signing.as_ref().map(|(agent, _)| json!({ "agent": agent })),
        "aliases": [],
    });
    let mut unserved = 0usize;
    let mut say = |line: String, section: &str, entry: Value| {
        if !args.json {
            println!("{line}");
        }
        match report[section].as_array_mut() {
            Some(list) => list.push(entry),
            None => report[section] = entry,
        }
    };
    let mut base = None;
    for authority in candidates(&settings, &home) {
        match client
            .get(format!("http://{authority}/health"))
            .send()
            .await
        {
            Ok(answer) => {
                let status = answer.status().as_u16();
                say(
                    format!("health {status} at {authority}"),
                    "addresses",
                    json!({ "address": authority, "health": status }),
                );
                base = Some(format!("http://{authority}"));
                break;
            }
            Err(error) => say(
                format!("{authority}: {error}"),
                "addresses",
                json!({ "address": authority, "error": error.to_string() }),
            ),
        }
    }
    let base = base.ok_or("no candidate address served /health")?;
    let requested = aliases.len();
    for alias in aliases {
        let body = serde_json::to_vec(
            &json!({ "model": alias, "messages": [{ "role": "user", "content": "say ok" }] }),
        )
        .expect("payload serialises");
        let mut request = client
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth(&token)
            .header("Content-Type", "application/json");
        if let Some((agent, secret)) = &signing {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or_default()
                .to_string();
            let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
                .map_err(|error| error.to_string())?;
            mac.update(
                format!(
                    "{agent}:{stamp}:{}",
                    hex::encode(Sha256::digest(&body))
                )
                .as_bytes(),
            );
            request = request
                .header("x-agent-id", agent.as_str())
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
                    say(
                        format!("{alias} {} model={served} said={said:?}", status.as_u16()),
                        "aliases",
                        json!({ "alias": alias, "status": status.as_u16(), "model": served, "said": said }),
                    );
                } else {
                    unserved += 1;
                    let refusal = text.trim().replace('\n', " ");
                    say(
                        format!("{alias} {} {refusal}", status.as_u16()),
                        "aliases",
                        json!({ "alias": alias, "status": status.as_u16(), "refusal": refusal }),
                    );
                }
            }
            Err(error) => {
                unserved += 1;
                say(
                    format!("{alias} unreachable {error}"),
                    "aliases",
                    json!({ "alias": alias, "error": error.to_string() }),
                )
            }
        }
    }
    if args.json {
        crate::cli::print_json(&report);
    }
    if unserved > 0 {
        return Err(format!(
            "{unserved} of {requested} alias(es) were not served; each line above names the refusal or the transport error"
        ));
    }
    Ok(())
}

pub(crate) async fn run(args: ProbeArgs) {
    if !args.allow_provider_cost {
        eprintln!("refusing billable probe requests without explicit --allow-provider-cost");
        std::process::exit(2);
    }
    if let Err(detail) = probe(args).await {
        eprintln!("{detail}");
        std::process::exit(1);
    }
}
