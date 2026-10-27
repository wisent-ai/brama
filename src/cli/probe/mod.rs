//! `brama probe`: ask the running gateway on this host to serve one real
//! request per alias and report what came back.
//!
//! `/health` answering says the process started; it says nothing about
//! whether a provider credential can be redeemed, which is the failure this
//! command exists for: healthy and serving nothing. Each alias is a real
//! completion through the gateway's own listener, so a line with a body means
//! a model answered. It makes requests and changes nothing. The caller names
//! the client identity it probes as; no item, agent or alias is built in, and
//! the bearer and signing secret are read by the vault role their items play.

use std::collections::BTreeMap;
use std::path::Path;

use clap::Args;
use serde_json::{json, Value};

use super::launcher::identities::vault::role_field_in;
use super::workload::{home, service_settings};

#[derive(Args)]
pub(crate) struct ProbeArgs {
    /// Alias to request; repeat for each. Without one, every alias the
    /// gateway declares is requested.
    #[arg(long = "alias")]
    aliases: Vec<String>,
    /// `<role>#<field>` of the client bearer the requests present: the item
    /// tagged `stado:role:<role>`, read through the entitlements router
    /// service.env names
    #[arg(long, value_name = "ROLE#FIELD")]
    bearer_role: String,
    /// Agent id every request is signed as; needs --signing-role
    #[arg(long, value_name = "AGENT", requires = "signing_role")]
    agent: Option<String>,
    /// `<role>#<field>` of the request-sign secret that signs each request as
    /// --agent; needs --agent
    #[arg(long, value_name = "ROLE#FIELD", requires = "agent")]
    signing_role: Option<String>,
    /// Acknowledge that each alias spends one short provider request
    #[arg(long, default_value_t = false)]
    allow_provider_cost: bool,
    /// Print the addresses tried, request signing and every alias's answer as
    /// one JSON document instead of lines
    #[arg(long, default_value_t = false)]
    json: bool,
}

/// What the gateway logs when it binds, followed by the address.
const ANNOUNCEMENT: &str = "Starting brama server on ";

/// One field of the item playing a role, read through the router with the
/// environment the launcher builds from service.env: without it the router
/// reports "vault not initialized" about a vault that is fine.
fn vault_field(
    router: &str,
    settings: &BTreeMap<String, String>,
    coordinate: &str,
) -> Result<String, String> {
    let (role, field) = coordinate
        .split_once('#')
        .filter(|(role, field)| !role.is_empty() && !field.is_empty())
        .ok_or_else(|| format!("`{coordinate}` is not `<role>#<field>`"))?;
    role_field_in(Path::new(router), settings, role, field)
}

/// The port service.env declares, on loopback, then the address the last boot
/// announced; plain HTTP is refused off loopback and the log may name an older
/// bind, so neither alone is reliable. The launcher refuses to start without
/// `PORT`, so no port is assumed here either: without one, only the announced
/// address is tried, as `brama diagnose` does. A log that was never written
/// announces nothing; one that exists but cannot be read is refused, since
/// skipping it would probe without the address it announces.
fn candidates(settings: &BTreeMap<String, String>, home: &Path) -> Result<Vec<String>, String> {
    let mut candidates: Vec<String> = settings
        .get("PORT")
        .map(|port| format!("{}:{port}", std::net::Ipv4Addr::LOCALHOST))
        .into_iter()
        .collect();
    let path = home
        .join(".stado/logs")
        .join(format!("{}.log", super::diagnose::SERVICE_LABEL));
    let log = match std::fs::read(&path) {
        Ok(log) => log,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
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
    Ok(candidates)
}

async fn probe(args: ProbeArgs) -> Result<(), String> {
    let home = home()?;
    let settings = service_settings(&home)?;
    let router = settings
        .get("ENTITLEMENTS_ROUTER_BIN")
        .cloned()
        .ok_or("service env names no entitlements router")?;
    let token = vault_field(&router, &settings, &args.bearer_role).map_err(|error| {
        format!(
            "cannot read the bearer {} through {router}: {error}",
            args.bearer_role
        )
    })?;
    // The secret stays in this process: never printed, never in argv.
    let signing = match (&args.agent, &args.signing_role) {
        (Some(agent), Some(coordinate)) => {
            let secret = vault_field(&router, &settings, coordinate)
                .map_err(|error| format!("cannot read {coordinate} to sign as {agent}: {error}"))?;
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
    let mut unserved: Vec<String> = Vec::new();
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
    for authority in candidates(&settings, &home)? {
        match stado_wait::request(client.get(format!("http://{authority}/health"))).await {
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
    let base = base.ok_or(
        "no candidate address served /health: service.env names no PORT and the gateway \
         announced no address, or none answered (each tried address is listed above)",
    )?;
    let requested = aliases.len();
    for alias in aliases {
        let body = serde_json::to_vec(
            &json!({ "model": alias, "messages": [{ "role": "user", "content": "say ok" }] }),
        )
        .map_err(|error| format!("the probe request for {alias} cannot be encoded: {error}"))?;
        let mut request = client
            .post(format!("{base}/v1/chat/completions"))
            .bearer_auth(&token)
            .header("Content-Type", "application/json");
        if let Some((agent, secret)) = &signing {
            let headers = brama::crypto::hmac_auth::agent_headers(agent, &body, secret.as_bytes())
                .map_err(|error| format!("cannot sign the probe request: {error}"))?;
            for (name, value) in headers {
                request = request.header(name, value);
            }
        }
        match stado_wait::request(request.body(body)).await {
            Ok(answer) => {
                let status = answer.status();
                // An answer whose body cannot be read, or a success that is not
                // JSON, did not serve the alias: it is a refusal with its cause.
                let outcome: Result<Value, String> = match answer.text().await {
                    Err(error) => Err(format!("unreadable answer: {}", error.without_url())),
                    Ok(text) if !status.is_success() => Err(text.trim().replace('\n', " ")),
                    Ok(text) => serde_json::from_str(&text).map_err(|error| {
                        format!(
                            "answer is not JSON ({error}): {}",
                            text.trim().replace('\n', " ")
                        )
                    }),
                };
                // A success whose first choice carries no text did not serve
                // the alias either, never a quiet model printed as said="".
                let outcome = outcome.and_then(|payload| {
                    let said = payload
                        .pointer("/choices/0/message/content")
                        .and_then(Value::as_str)
                        .map(|said| said.trim().to_string());
                    match said {
                        Some(said) if !said.is_empty() => Ok((said, payload)),
                        _ => Err(format!(
                            "answer carries no nonblank text content: {payload}"
                        )),
                    }
                });
                match outcome {
                    Ok((said, payload)) => {
                        let served = payload.get("model").and_then(Value::as_str);
                        let shown = match served {
                            Some(model) => model,
                            None => "(not stated)",
                        };
                        say(
                            format!("{alias} {} model={shown} said={said:?}", status.as_u16()),
                            "aliases",
                            json!({ "alias": alias, "status": status.as_u16(), "model": served, "said": said }),
                        );
                    }
                    Err(refusal) => {
                        unserved.push(alias.clone());
                        say(
                            format!("{alias} {} {refusal}", status.as_u16()),
                            "aliases",
                            json!({ "alias": alias, "status": status.as_u16(), "refusal": refusal }),
                        );
                    }
                }
            }
            Err(error) => {
                unserved.push(alias.clone());
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
    if !unserved.is_empty() {
        return Err(format!(
            "{} of {requested} alias(es) were not served ({}); each line above names the refusal or the transport error",
            unserved.len(),
            unserved.join(", ")
        ));
    }
    Ok(())
}

pub(crate) async fn run(args: ProbeArgs) {
    if !args.allow_provider_cost {
        crate::cli::refuse_unacknowledged_cost("billable probe requests");
    }
    if let Err(detail) = probe(args).await {
        crate::cli::refused(detail);
    }
}
