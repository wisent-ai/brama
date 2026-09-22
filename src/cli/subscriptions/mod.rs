//! `brama subscriptions`: the subscription pool this deployment routes over,
//! and each provider's own usage report when it is asked for.
//!
//! The pool an operator needs to read is usually not this process's: the
//! gateway runs on the fleet's host and the operator is at a workstation. So
//! the same report is readable from here against that gateway, resolved and
//! authenticated through Stado, and every member says who refreshes its
//! grant.

pub(crate) mod credentials;
mod manual;
mod membership;
pub(crate) mod remote;
pub(crate) mod unattended;
pub(crate) mod verdicts;

use clap::Args;
use serde_json::Value;

#[derive(Args)]
pub(crate) struct SubscriptionsArgs {
    /// Print the report as JSON instead of lines
    #[arg(long, default_value_t = false)]
    json: bool,
    /// Read current free provider usage reports; never starts a sign-in or model request
    #[arg(long, default_value_t = false)]
    refresh_usage: bool,
    /// Apply a pool membership document from stdin; uses the same bank/retire contract as HTTP
    #[arg(long, conflicts_with = "refresh_usage")]
    apply: bool,
    /// Read the pool of the gateway at this origin instead of this process's own
    #[arg(long, conflicts_with_all = ["apply", "gateway_consumer"])]
    gateway: Option<String>,
    /// Read the pool of the gateway Stado's service directory gives this consumer
    #[arg(long, conflicts_with = "apply")]
    gateway_consumer: Option<String>,
    /// Read the console's bearer from the vault as `<item>#<field>` instead of from stdin
    #[arg(long, conflicts_with = "apply")]
    bearer_item: Option<String>,
}

// The operator's own console: this process holds the vault and the
// ledger, so the deployment scope is what it can prove.
pub(crate) async fn report(args: SubscriptionsArgs) {
    let SubscriptionsArgs {
        json,
        refresh_usage,
        apply,
        gateway,
        gateway_consumer,
        bearer_item,
    } = args;
    if apply {
        let body = match std::io::read_to_string(std::io::stdin()) {
            Ok(body) => body,
            Err(error) => {
                eprintln!("cannot read subscription membership from stdin: {error}");
                std::process::exit(1);
            }
        };
        match brama::core::server::apply_subscription_membership(body.as_bytes()).await {
            Ok(receipt) => crate::cli::print_json(&receipt),
            Err(error) => {
                crate::cli::print_json(&error);
                std::process::exit(1);
            }
        }
        return;
    }
    let destination = remote::Destination {
        gateway,
        gateway_consumer,
        bearer_item,
    };
    if destination.gateway.is_some() || destination.gateway_consumer.is_some() {
        remote_report(destination, refresh_usage, json).await;
        return;
    }
    if destination.bearer_item.is_some() {
        eprintln!("--bearer-item is for a gateway; name --gateway or --gateway-consumer");
        std::process::exit(1);
    }
    let scope = brama::subscription_dispatch::pool::PoolScope::Deployment;
    // Two capabilities, one per question, and the same document from
    // either: the pool states what this deployment has recorded, plan
    // usage reads each provider's own usage report first.
    let report = if refresh_usage {
        brama::subscription_dispatch::plan_usage::report(&scope).await
    } else {
        brama::subscription_dispatch::pool::report(&scope).await
    };
    if json {
        crate::cli::print_json(&report);
    } else {
        print_pool(&report);
    }
    if report.get("ok").and_then(Value::as_bool) != Some(true) {
        std::process::exit(1);
    }
}

/// Read one gateway's own pool report and print it exactly as the local one
/// is printed.
///
/// Usage reports are the gateway's to fetch, so `--refresh-usage` is refused
/// here by name rather than answered with a report that did not do it.
async fn remote_report(destination: remote::Destination, refresh_usage: bool, json: bool) {
    if refresh_usage {
        eprintln!(
            "--refresh-usage reads providers from the gateway that holds the credentials; run it there, or read this gateway's recorded pool without it"
        );
        std::process::exit(1);
    }
    let mut stdin_bearer = zeroize::Zeroizing::new(String::new());
    if destination.bearer_item.is_none() {
        if let Err(error) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin_bearer)
        {
            eprintln!("reading the console bearer from stdin: {error}");
            std::process::exit(1);
        }
    }
    let (origin, bearer) = match destination.resolve(&stdin_bearer).await {
        Ok(resolved) => resolved,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let Some(origin) = origin else {
        eprintln!("name --gateway or --gateway-consumer to read another gateway's pool");
        std::process::exit(1);
    };
    let report = match remote::pool_report(&origin, bearer.trim()).await {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    if json {
        crate::cli::print_json(&report);
    } else {
        println!("pool of the gateway at {origin}");
        print_pool(&report);
    }
    if report.get("ok").and_then(Value::as_bool) != Some(true) {
        std::process::exit(1);
    }
}

mod report;

use report::{print_pool, text};
