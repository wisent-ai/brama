//! `brama maintain`: one maintenance pass of a serving gateway.
//!
//! The serving gateway runs its own maintenance passes (one when it starts,
//! one at each moment its ledger states something becomes due, one after a
//! refusal for want of a signed-in subscription); this command asks for one
//! more now. The pass runs in the serving process because that process holds
//! the usage ledger, the journal and the credentials the launcher installed.

use clap::{ArgGroup, Args};
use serde_json::Value;

use crate::cli::subscriptions::remote::{self, Destination};

#[derive(Args)]
#[command(group(
    ArgGroup::new("destination")
        .required(true)
        .args(["gateway", "gateway_consumer"])
))]
pub(crate) struct MaintainArgs {
    /// The gateway to maintain; the console's bearer is read from stdin
    #[arg(long)]
    gateway: Option<String>,
    /// Resolve the gateway through Stado's service directory as this consumer
    #[arg(long)]
    gateway_consumer: Option<String>,
    /// Read the console's bearer from the vault item playing this role (its
    /// `token` field) instead of from stdin
    #[arg(long, value_name = "ROLE")]
    bearer_role: Option<String>,
    /// Print the gateway's report as JSON instead of lines
    #[arg(long, default_value_t = false)]
    json: bool,
}

pub(crate) async fn run(args: MaintainArgs) {
    let destination = Destination {
        gateway: args.gateway,
        gateway_consumer: args.gateway_consumer,
        bearer_role: args.bearer_role,
    };
    let report = match pass(destination).await {
        Ok(report) => report,
        Err(error) => crate::cli::refused(error),
    };
    if args.json {
        crate::cli::print_json(&report);
    } else {
        print_report(&report);
    }
    if report.get("ok").and_then(Value::as_bool) != Some(true) {
        crate::cli::fail();
    }
}

async fn pass(destination: Destination) -> Result<Value, String> {
    let (origin, bearer) = destination.resolve_reading_stdin().await?;
    let origin = origin.ok_or_else(|| {
        String::from(
            "name --gateway or --gateway-consumer: the pass runs in the serving gateway, \
             which holds the usage ledger and the credentials",
        )
    })?;
    remote::maintain(&origin, bearer.trim()).await
}

fn print_report(report: &Value) {
    // A field the gateway's report left out is named as missing, not printed
    // as a value.
    let count = |pointer: &str| match report.pointer(pointer) {
        Some(value) => value.to_string(),
        None => format!("(the gateway did not report {pointer})"),
    };
    if let Some(accounts) = report
        .pointer("/discovery/accounts")
        .and_then(Value::as_array)
    {
        for account in accounts {
            println!(
                "account discovery: {} {} registered={} source={}",
                account["provider"], account["account"], account["registered"], account["source"]
            );
        }
    }
    if let Some(errors) = report
        .pointer("/discovery/errors")
        .and_then(Value::as_array)
    {
        for error in errors {
            println!("account discovery failed: {error}");
        }
    }
    println!(
        "plan usage: {} subscription(s), {} read",
        count("/plan_usage/subscriptions"),
        count("/plan_usage/read")
    );
    if let Some(failed) = report
        .pointer("/plan_usage/failed")
        .and_then(Value::as_array)
    {
        for failure in failed {
            // A failure row without its subscription or provider is named as
            // such; "?" read like part of an id.
            let said = |field: &str| match failure.get(field).and_then(Value::as_str) {
                Some(text) => text,
                None => "not reported",
            };
            println!(
                "  failed {} ({}): {}",
                said("subscription"),
                said("provider"),
                match failure.get("error") {
                    Some(error) => error.clone(),
                    None => Value::String("no error was reported".to_string()),
                }
            );
        }
    }
    if let Some(error) = report.pointer("/attribution/error") {
        println!("account attribution: {error}");
    }
    for provider in report
        .pointer("/attribution/failed")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        println!(
            "account attribution failed for {provider}: {}",
            report["attribution"]["providers"][provider]["error"]
        );
    }
    match report.pointer("/credentials/error").and_then(Value::as_str) {
        Some(error) => println!("credentials: {error}"),
        None => println!(
            "credentials: {} subscription(s), {} refreshed, {} refused, {} sign-in check(s) scheduled",
            count("/credentials/subscriptions"),
            count("/credentials/refreshed"),
            count("/credentials/refused"),
            count("/credentials/sign_in_checks_scheduled")
        ),
    }
    if let Some(error) = report.pointer("/resets/error") {
        println!("saved reset maintenance failed: {error}");
    }
    if let Some(members) = report.pointer("/resets/members").and_then(Value::as_array) {
        for member in members {
            println!(
                "saved reset: {} ({}) {}",
                member["member"], member["provider"], member
            );
        }
    }
    if let Some(detail) = report.pointer("/acquisition/detail") {
        println!("acquisition: {detail}");
    }
    for verdict in report["acquisition"]["verdicts"]
        .as_array()
        .into_iter()
        .flatten()
    {
        println!(
            "acquisition: {} {} ({}): {}",
            verdict["provider"], verdict["result"], verdict["code"], verdict["detail"]
        );
    }
    let reason = match report.pointer("/readiness/reason").and_then(Value::as_str) {
        Some(reason) => reason.to_string(),
        None => "(no reason reported)".to_string(),
    };
    println!("readiness: ready={} {reason}", count("/readiness/ready"));
}
