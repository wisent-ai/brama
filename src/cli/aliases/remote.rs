//! `brama routes set|rm` on the gateway that serves, through
//! `PUT` and `DELETE /v1/admin/routes`.
//!
//! The route registry is the gateway's own file on the gateway's host, so a
//! route written from a workstation without a gateway named lands in a file
//! no serving process reads. Naming the gateway sends the change to the
//! process that serves the aliases, which validates it against what it can
//! serve and writes its own registry atomically, the same path Brama Desktop
//! takes. This is the one way to change a served route from another machine.

use serde_json::{json, Value};

use crate::cli::subscriptions::remote::Destination;

/// The gateway a route change is sent to instead of a local registry file.
#[derive(clap::Args)]
pub(crate) struct GatewayArgs {
    /// The gateway that serves the aliases: the change goes through its admin API, and the console's bearer is read from stdin unless --bearer-role names it
    #[arg(long, conflicts_with_all = ["file", "gateway_consumer"])]
    gateway: Option<String>,
    /// Resolve the gateway through Stado's service directory as this consumer
    #[arg(long, conflicts_with = "file")]
    gateway_consumer: Option<String>,
    /// Read the console's bearer from the vault item playing this role (its `token` field) instead of from stdin
    #[arg(long, value_name = "ROLE")]
    bearer_role: Option<String>,
}

impl GatewayArgs {
    /// The gateway to send the change to, or `None` for the local registry
    /// file. A bearer role alone is a destination too, so its refusal (no
    /// gateway named) is said instead of the role being ignored.
    pub(super) fn destination(self) -> Option<Destination> {
        let named = self.gateway.is_some() || self.gateway_consumer.is_some() || self.bearer_role.is_some();
        named.then_some(Destination {
            gateway: self.gateway,
            gateway_consumer: self.gateway_consumer,
            bearer_role: self.bearer_role,
        })
    }
}

/// What one route change on the gateway asks.
pub(super) enum Change {
    Set { alias: String, destination: String },
    Remove { alias: String },
}

/// Send `change` to the gateway `destination` names and print its answer the
/// way the local commands print theirs, or say why it was refused: the
/// gateway's status and whole answer.
pub(super) async fn apply(destination: Destination, change: Change, json: bool) -> Result<(), String> {
    let (origin, answer) = send(destination, &change).await?;
    let line = match &change {
        Change::Set { alias, destination } => format!("{alias} -> {destination}"),
        Change::Remove { alias } => format!("{alias} removed"),
    };
    if json {
        crate::cli::print_json(&json!({
            "gateway": origin,
            "change": line,
            "routes": answer.get("routes"),
        }));
        return Ok(());
    }
    println!("gateway: {origin}");
    println!("change: {line}");
    for (alias, destination) in answer.get("routes").and_then(Value::as_object).into_iter().flatten() {
        match destination.as_str() {
            Some(text) => println!("{alias:<32} {text}"),
            None => println!("{alias:<32} {destination}"),
        }
    }
    Ok(())
}

async fn send(destination: Destination, change: &Change) -> Result<(String, Value), String> {
    let (origin, bearer) = destination.resolve_reading_stdin().await?;
    let origin = origin.ok_or("name --gateway or --gateway-consumer to change a served route")?;
    let url = format!("{}/v1/admin/routes", origin.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| format!("gateway client: {error}"))?;
    let (request, action) = match change {
        Change::Set { alias, destination } => (
            client
                .put(&url)
                .json(&json!({"alias": alias, "primary": destination})),
            format!("point {alias} at {destination}"),
        ),
        Change::Remove { alias } => (
            client.delete(&url).json(&json!({"alias": alias})),
            format!("remove {alias}"),
        ),
    };
    let response = request
        .bearer_auth(bearer.trim())
        .send()
        .await
        .map_err(|error| format!("the gateway at {url} did not answer: {error}"))?;
    let status = response.status();
    let answer: Value = response
        .json()
        .await
        .map_err(|error| format!("the gateway's answer to {action} ({status}) is not JSON: {error}"))?;
    if !status.is_success() {
        return Err(format!("the gateway refused to {action}: {status}: {answer}"));
    }
    Ok((origin, answer))
}
