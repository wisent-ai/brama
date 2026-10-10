//! Discover local account identities and enroll them at the serving gateway.

use super::remote::Destination;
use clap::{ArgGroup, Args};
use serde_json::Value;

#[derive(Args)]
#[command(group(ArgGroup::new("destination").args(["gateway", "gateway_consumer"])))]
pub(crate) struct DiscoveryArgs {
    /// Enroll accounts at this gateway rather than this process's own vault
    #[arg(long)]
    gateway: Option<String>,
    /// Resolve the serving gateway through Stado as this consumer
    #[arg(long)]
    gateway_consumer: Option<String>,
    /// Read the console bearer by role; otherwise read it from stdin
    #[arg(long, requires = "destination")]
    bearer_role: Option<String>,
    /// Print the complete report as JSON
    #[arg(long)]
    json: bool,
}

pub(crate) async fn run(args: DiscoveryArgs) {
    let result = execute(Destination {
        gateway: args.gateway,
        gateway_consumer: args.gateway_consumer,
        bearer_role: args.bearer_role,
    })
    .await;
    match result {
        Ok(report) => {
            crate::cli::print_answer(&report, args.json);
            if report["ok"].as_bool() != Some(true) {
                std::process::exit(libc::EXIT_FAILURE);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(libc::EXIT_FAILURE);
        }
    }
}

async fn execute(destination: Destination) -> Result<Value, String> {
    let (gateway, bearer) = destination.resolve_reading_stdin().await?;
    let report = brama::subscription_dispatch::discovery::harness::omp().await;
    let Some(gateway) = gateway else {
        return Ok(brama::subscription_dispatch::discovery::discover(report).await);
    };
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| error.to_string())?;
    let response = stado_wait::http::request(
        client
            .post(format!(
                "{}/v1/admin/subscription-pool/discover",
                gateway.trim_end_matches('/')
            ))
            .bearer_auth(bearer.trim())
            .json(&report),
    )
    .await
    .map_err(|error| format!("gateway account discovery request: {error}"))?;
    let status = response.status();
    let answer: Value = stado_wait::until(
        stado_wait::Kind::Network,
        "decode account discovery response",
        &gateway,
        response.json(),
    )
    .await
    .map_err(|error| format!("gateway account discovery HTTP {status}: {error}"))?;
    if !status.is_success() {
        return Err(format!("gateway account discovery HTTP {status}: {answer}"));
    }
    Ok(answer)
}
