//! `brama subscription lease take|release|list`: which live session runs on
//! which subscription, on the gateway that holds the pool. The register is the
//! gateway's, because every machine's sessions share one pool; a lease taken
//! here without a gateway would be a claim nobody else could see.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Value};

use super::remote;
use brama::subscription_dispatch::leases::{Lease, Taken};

/// End the command unsuccessfully, as every Brama verb does.
fn fail() -> ! {
    std::process::exit(1) // https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/stdlib.h.html
}

#[derive(Deserialize)]
struct Released {
    released: Vec<Lease>,
}

#[derive(Deserialize)]
struct Listed {
    limits: BTreeMap<String, u64>,
    unstated: BTreeMap<String, String>,
    leases: Vec<Lease>,
    counts: BTreeMap<String, u64>,
}

/// One request to the gateway's lease routes, waited on through the shared
/// wait so stderr says what is asked of which gateway; the body it answered,
/// or the refusal with its status and the whole error document as the
/// gateway wrote it, so the code, the message and the details reach the
/// reader unchanged.
async fn call(
    destination: remote::Destination,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let (origin, bearer) = destination.resolve_reading_stdin().await?;
    let origin = origin.ok_or_else(|| {
        String::from(
            "a lease lives on the gateway that holds the pool: name --gateway or --gateway-consumer",
        )
    })?;
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| format!("gateway client: {error}"))?;
    let mut request = client
        .request(method, format!("{}{path}", origin.trim_end_matches('/')))
        .bearer_auth(bearer.trim());
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = stado_wait::http::request(request)
        .await
        .map_err(|error| format!("the gateway {origin} did not answer: {error}"))?;
    let status = response.status();
    let answer: Value = stado_wait::until(
        stado_wait::Kind::Network,
        format!("the body of the gateway's HTTP {status} answer to {path}"),
        origin.clone(),
        response.json(),
    )
    .await
    .map_err(|error| {
        format!("the gateway {origin} answered HTTP {status} with an unreadable body: {error}")
    })?;
    if !status.is_success() {
        return Err(format!(
            "the gateway refused: HTTP {}: {}",
            status.as_u16(),
            answer["error"]
        ));
    }
    Ok(answer)
}

fn answered<T: serde::de::DeserializeOwned>(answer: Value, what: &str) -> T {
    match serde_json::from_value(answer) {
        Ok(read) => read,
        Err(error) => {
            eprintln!(
                "the gateway's {what} answer is not the document this command reads: {error}"
            );
            fail();
        }
    }
}

fn print_lease(lease: &Lease) {
    println!(
        "{}  {}  {} ({})  session {}  held by {}  since {}",
        lease.id,
        lease.provider,
        lease.subscription_id,
        match &lease.account {
            Some(account) => account.as_str(),
            None => "no account declared",
        },
        lease.session_id,
        lease.holder,
        lease.taken_at_ms
    );
}

pub(crate) async fn take(
    destination: remote::Destination,
    provider: &str,
    session_id: &str,
    holder: &str,
    json: bool,
) {
    let taken = call(
        destination,
        reqwest::Method::POST,
        "/v1/subscription-pool/leases",
        Some(json!({"provider": provider, "session_id": session_id, "holder": holder})),
    )
    .await;
    let taken = match taken {
        Ok(taken) => taken,
        Err(error) => {
            eprintln!("{error}");
            fail();
        }
    };
    if json {
        crate::cli::print_json(&taken);
        return;
    }
    let taken: Taken = answered(taken, "lease");
    print_lease(&taken.lease);
    println!(
        "{}: {} of {} sessions on {}",
        if taken.new { "taken" } else { "already held" },
        taken.live_on_subscription,
        taken.limit,
        taken.lease.subscription_id
    );
}

pub(crate) async fn release(
    destination: remote::Destination,
    lease_id: Option<&str>,
    session_id: Option<&str>,
    json: bool,
) {
    let released = call(
        destination,
        reqwest::Method::DELETE,
        "/v1/subscription-pool/leases",
        Some(json!({"lease_id": lease_id, "session_id": session_id})),
    )
    .await;
    let released = match released {
        Ok(released) => released,
        Err(error) => {
            eprintln!("{error}");
            fail();
        }
    };
    if json {
        crate::cli::print_json(&released);
        return;
    }
    let released: Released = answered(released, "release");
    if released.released.is_empty() {
        println!("nothing was live under that name; nothing released");
        return;
    }
    for lease in &released.released {
        print_lease(lease);
    }
    println!("released {}", released.released.len());
}

pub(crate) async fn list(destination: remote::Destination, json: bool) {
    let listed = call(
        destination,
        reqwest::Method::GET,
        "/v1/subscription-pool/leases",
        None,
    )
    .await;
    let listed = match listed {
        Ok(listed) => listed,
        Err(error) => {
            eprintln!("{error}");
            fail();
        }
    };
    if json {
        crate::cli::print_json(&listed);
        return;
    }
    let listed: Listed = answered(listed, "lease list");
    for (provider, limit) in &listed.limits {
        println!("{provider}: at most {limit} sessions per subscription");
    }
    for (provider, refusal) in &listed.unstated {
        println!("{provider}: no lease is given: {refusal}");
    }
    for lease in &listed.leases {
        print_lease(lease);
    }
    for (subscription, count) in &listed.counts {
        println!("{subscription} carries {count}");
    }
}
