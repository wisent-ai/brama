//! A reset consumes already-owned provider capacity; it never purchases an account.

use super::remote::Destination;
use clap::{ArgGroup, Args, Subcommand};
use serde::Serialize;
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
    let address = format!(
        "{}/v1/admin/subscription-pool/reset",
        gateway.trim_end_matches('/')
    );
    let response =
        stado_wait::request(client.post(&address).bearer_auth(bearer.trim()).json(
            &json!({"provider": args.provider, "member": args.member, "reason": args.reason}),
        ))
        .await
        .map_err(|error| format!("POST {address}: {error}"))?;
    gateway_answer(response, &address).await
}

/// Where a reset-policy command is answered: this host's Brama state, or the
/// gateway that holds the pool.
#[derive(Args)]
#[command(group(ArgGroup::new("destination").args(["gateway", "gateway_consumer"])))]
pub(crate) struct PolicyTarget {
    #[arg(long)]
    gateway: Option<String>,
    #[arg(long)]
    gateway_consumer: Option<String>,
    #[arg(long, requires = "destination")]
    bearer_role: Option<String>,
    #[arg(long)]
    json: bool,
}

/// The operator's policy for redeeming a provider's saved resets automatically.
#[derive(Subcommand)]
pub(crate) enum PolicyCommand {
    /// Every provider's stated policy
    Show {
        #[command(flatten)]
        target: PolicyTarget,
    },
    /// State one provider's policy, replacing the one before
    Set {
        provider: String,
        /// Credits never spent automatically
        #[arg(long)]
        keep_credits: u64,
        /// A window must stay blocked longer than this before a credit is spent on it
        #[arg(long)]
        min_blocked_minutes: u64,
        /// A credit expiring within this many hours is spent rather than lost
        #[arg(long)]
        salvage_horizon_hours: u64,
        #[arg(long)]
        reason: String,
        #[command(flatten)]
        target: PolicyTarget,
    },
    /// Remove one provider's policy, so its resets are no longer redeemed automatically
    Clear {
        provider: String,
        #[command(flatten)]
        target: PolicyTarget,
    },
}

/// One reset-policy operation, in the document the gateway's
/// `/v1/admin/subscription-pool/reset-policy` route reads.
#[derive(Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum PolicyRequest {
    Show,
    Set {
        provider: String,
        keep_credits: u64,
        min_blocked_minutes: u64,
        salvage_horizon_hours: u64,
        reason: String,
    },
    Clear {
        provider: String,
    },
}

pub(crate) async fn policy(command: PolicyCommand) {
    let (target, request) = match command {
        PolicyCommand::Show { target } => (target, PolicyRequest::Show),
        PolicyCommand::Set {
            provider,
            keep_credits,
            min_blocked_minutes,
            salvage_horizon_hours,
            reason,
            target,
        } => (
            target,
            PolicyRequest::Set {
                provider,
                keep_credits,
                min_blocked_minutes,
                salvage_horizon_hours,
                reason,
            },
        ),
        PolicyCommand::Clear { provider, target } => (target, PolicyRequest::Clear { provider }),
    };
    match policy_answer(&target, request).await {
        Ok(answer) => crate::cli::print_answer(&answer, target.json),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(libc::EXIT_FAILURE);
        }
    }
}

/// Answer `request` from this host's Brama state, or ask the gateway that
/// holds the pool.
async fn policy_answer(target: &PolicyTarget, request: PolicyRequest) -> Result<Value, String> {
    let destination = Destination {
        gateway: target.gateway.clone(),
        gateway_consumer: target.gateway_consumer.clone(),
        bearer_role: target.bearer_role.clone(),
    };
    let (gateway, bearer) = destination.resolve_reading_stdin().await?;
    let Some(gateway) = gateway else {
        return local_policy(request);
    };
    let client = reqwest::Client::builder()
        .build()
        .map_err(|error| error.to_string())?;
    let address = format!(
        "{}/v1/admin/subscription-pool/reset-policy",
        gateway.trim_end_matches('/')
    );
    let response = stado_wait::request(
        client
            .post(&address)
            .bearer_auth(bearer.trim())
            .json(&request),
    )
    .await
    .map_err(|error| format!("POST {address}: {error}"))?;
    gateway_answer(response, &address).await
}

async fn gateway_answer(response: reqwest::Response, address: &str) -> Result<Value, String> {
    let status = response.status();
    let body = stado_wait::until(
        stado_wait::Kind::Network,
        "read gateway reset response",
        address,
        response.text(),
    )
    .await
    .map_err(|error| format!("POST {address}: HTTP {status}: reading response: {error}"))?;
    if !status.is_success() {
        return Err(format!("POST {address}: HTTP {status}: {body}"));
    }
    serde_json::from_str(&body)
        .map_err(|error| format!("POST {address}: HTTP {status}: invalid JSON: {error}"))
}

/// The same three operations on this host's Brama state, answered in the
/// gateway route's shape.
fn local_policy(request: PolicyRequest) -> Result<Value, String> {
    use brama::subscription_dispatch::acquire::resets::settings;
    match request {
        PolicyRequest::Show => settings::all().map(|policies| json!({ "policies": policies })),
        PolicyRequest::Set {
            provider,
            keep_credits,
            min_blocked_minutes,
            salvage_horizon_hours,
            reason,
        } => {
            let policy = settings::ResetPolicy {
                keep_credits,
                min_blocked_minutes,
                salvage_horizon_hours,
                reason,
                stated_at: chrono::Utc::now().to_rfc3339(),
            };
            settings::set(&provider, policy)
                .map(|policy| json!({ "provider": provider, "policy": policy }))
        }
        PolicyRequest::Clear { provider } => settings::clear(&provider)
            .map(|policy| json!({ "provider": provider, "cleared": policy })),
    }
}
