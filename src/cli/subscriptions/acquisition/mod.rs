//! `brama subscription acquire` and `brama subscription hand-over`: buying
//! an account when the pool is spent, here or on the gateway that holds the
//! vault, and signing the pool's accounts into a harness on this machine.

use serde_json::{json, Value};

use super::credentials::Harness;
use super::remote;
use brama::subscription_dispatch::acquire::{self, hand_over};

/// End the command unsuccessfully, as every Brama verb does.
fn fail() -> ! {
    std::process::exit(1) // https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/stdlib.h.html
}

/// Run one acquisition and print its verdict. Exits unsuccessfully unless an
/// account was bought: a refusal is an answer, but the caller asked for one.
pub(crate) async fn acquire(
    destination: remote::Destination,
    provider: &str,
    reason: &str,
    json: bool,
) {
    let verdict = match destination.resolve_reading_stdin().await {
        Err(error) => Err(error),
        Ok((Some(origin), bearer)) => on_gateway(&origin, bearer.trim(), provider, reason).await,
        Ok((None, _)) => {
            acquire::acquire_account(acquire::AcquireOptions {
                provider: provider.to_owned(),
                reason: reason.to_owned(),
                trigger: acquire::Trigger::Operator,
                progress: Some(std::sync::Arc::new(|event: &Value| {
                    if let Some(sentence) =
                        brama::subscription_dispatch::sign_in::progress_sentence(event)
                    {
                        eprintln!("{sentence}");
                    }
                })),
            })
            .await
        }
    };
    let verdict = match verdict {
        Ok(verdict) => verdict,
        Err(error) => {
            eprintln!("{error}");
            fail();
        }
    };
    if json {
        crate::cli::print_json(&verdict);
    } else {
        print_acquisition(&verdict);
    }
    if verdict["result"] != json!(acquire::ACQUIRED) {
        fail();
    }
}

async fn on_gateway(
    gateway: &str,
    bearer: &str,
    provider: &str,
    reason: &str,
) -> Result<Value, String> {
    let response = reqwest::Client::builder()
        .build()
        .map_err(|error| format!("gateway client: {error}"))?
        .post(format!(
            "{}/v1/admin/subscription-pool/acquire",
            gateway.trim_end_matches('/')
        ))
        .bearer_auth(bearer)
        .json(&json!({"provider": provider, "reason": reason}))
        .send()
        .await
        .map_err(|error| format!("the gateway {gateway} did not answer: {error}"))?;
    let status = response.status();
    let body: Value = response.json().await.map_err(|error| {
        format!("the gateway {gateway} answered HTTP {status} with an unreadable body: {error}")
    })?;
    if !status.is_success() {
        return Err(format!(
            "the gateway refused the acquisition of a {provider} account: HTTP {}: {}",
            status.as_u16(),
            body
        ));
    }
    Ok(body)
}

fn print_acquisition(verdict: &Value) {
    println!(
        "{} {}: {}",
        verdict["provider"], verdict["result"], verdict["detail"]
    );
    if let Some(accounts) = verdict["accounts"].as_array() {
        println!(
            "    accounts: {} of at most {}",
            accounts.len(),
            verdict["cap"]
        );
    }
    for standing in verdict["standings"].as_array().into_iter().flatten() {
        println!(
            "    {} {} {}",
            standing["standing"], standing["id"], standing["detail"]
        );
    }
    for field in ["plan_tier", "subscription_id", "account", "run_id"] {
        if let Some(value) = verdict[field].as_str() {
            println!("    {field}: {value}");
        }
    }
}

/// Sign every pool account the harness lacks into it and print each row.
/// Exits unsuccessfully unless the harness lists every account the pool holds.
pub(crate) async fn hand_over(provider: &str, harness: Harness, json: bool) {
    let harness = match harness {
        Harness::Omp => hand_over::Harness::Omp,
    };
    let progress: brama::subscription_dispatch::sign_in::Progress =
        std::sync::Arc::new(|event: &Value| {
            if let Some(sentence) = brama::subscription_dispatch::sign_in::progress_sentence(event)
            {
                eprintln!("{sentence}");
            }
        });
    let verdict = match hand_over::hand_over(provider, harness, Some(&progress)).await {
        Ok(verdict) => verdict,
        Err(error) => {
            eprintln!("{error}");
            fail();
        }
    };
    if json {
        crate::cli::print_json(&verdict);
    } else {
        for row in verdict["accounts"].as_array().into_iter().flatten() {
            println!("{} {} {}", row["result"], row["account"], row["detail"]);
        }
    }
    if verdict["ok"] != json!(true) {
        fail();
    }
}
