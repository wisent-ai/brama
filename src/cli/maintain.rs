//! `brama maintain`: one maintenance pass of a serving gateway.
//!
//! The gateway no longer keeps its own timers. Plan usage reports, credential
//! renewal and the readiness reading are each taken when this command asks,
//! and the host's Stado schedule decides how often it asks. The pass runs in
//! the serving process because that process holds the usage ledger, the
//! journal and the credentials the launcher installed.

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
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    if args.json {
        crate::cli::print_json(&report);
    } else {
        print_report(&report);
    }
    if report.get("ok").and_then(Value::as_bool) != Some(true) {
        std::process::exit(1);
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
    let count = |pointer: &str| report.pointer(pointer).cloned().unwrap_or(Value::Null);
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
            println!(
                "  failed {} ({}): {}",
                failure
                    .get("subscription")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
                failure
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or("?"),
                failure.get("error").cloned().unwrap_or(Value::Null)
            );
        }
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
    println!(
        "readiness: ready={} {}",
        count("/readiness/ready"),
        report
            .pointer("/readiness/reason")
            .and_then(Value::as_str)
            .unwrap_or("")
    );
}
