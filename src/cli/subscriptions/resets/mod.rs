//! A reset consumes already-owned provider capacity; it never purchases an account.

use super::remote::Destination;
use clap::{ArgGroup, Args};
use serde_json::{json, Value};

#[derive(Args)]
#[command(group(ArgGroup::new("destination").args(["gateway", "gateway_consumer"])))]
pub(crate) struct ResetArgs {
    provider: String,
    #[arg(long)]
    member: String,
    #[arg(long)]
    reason: String,
    #[arg(long)]
    gateway: Option<String>,
    #[arg(long)]
    gateway_consumer: Option<String>,
    #[arg(long, requires = "destination")]
    bearer_role: Option<String>,
    #[arg(long)]
    json: bool,
}

pub(crate) async fn run(args: ResetArgs) {
    let result = execute(&args).await;
    match result {
        Ok(answer) => {
            crate::cli::print_answer(&answer, args.json);
            if answer["ok"].as_bool() != Some(true) {
                std::process::exit(libc::EXIT_FAILURE);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(libc::EXIT_FAILURE);
        }
    }
}

async fn execute(args: &ResetArgs) -> Result<Value, String> {
    let destination = Destination {
        gateway: args.gateway.clone(),
        gateway_consumer: args.gateway_consumer.clone(),
        bearer_role: args.bearer_role.clone(),
    };
    let (gateway, bearer) = destination.resolve_reading_stdin().await?;
    let Some(gateway) = gateway else {
        return brama::subscription_dispatch::acquire::resets::redeem::run(
            &args.provider,
            &args.member,
            &args.reason,
            false,
        )
        .await;
    };
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| error.to_string())?;
    let response = stado_wait::http::request(
        client
            .post(format!(
                "{}/v1/admin/subscription-pool/reset",
                gateway.trim_end_matches('/')
            ))
            .bearer_auth(bearer.trim())
            .json(
                &json!({"provider": args.provider, "member": args.member, "reason": args.reason}),
            ),
    )
    .await
    .map_err(|error| format!("gateway reset request: {error}"))?;
    let status = response.status();
    let answer: Value = stado_wait::until(
        stado_wait::Kind::Network,
        "decode subscription reset response",
        &gateway,
        response.json(),
    )
    .await
    .map_err(|error| format!("gateway reset HTTP {status}: {error}"))?;
    if !status.is_success() {
        return Err(format!("gateway reset HTTP {status}: {answer}"));
    }
    Ok(answer)
}
