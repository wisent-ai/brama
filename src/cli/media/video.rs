//! `brama video start` and `brama video status`: the two halves of a render
//! that finishes minutes after it is asked for.
//!
//! The gateway keeps no job state, so `status` names the same model `start`
//! did. That is not an inconvenience to route around: a job identifier
//! belongs to the provider that issued it, and a gateway guessing which
//! vendor to ask would eventually ask the wrong one.

use clap::{Args, Subcommand};
use serde_json::{Map, Value};

use brama::core::server::VIDEO_ALIAS;
use brama::subscription_dispatch::{dispatch_direct_video, dispatch_direct_video_status};

use super::{job_json, job_lines, resolve_media_route};

#[derive(Subcommand)]
pub(crate) enum VideoCommand {
    /// Start one video job
    Start(StartArgs),
    /// Read one started job back from the provider that issued it
    Status(StatusArgs),
}

#[derive(Args)]
pub(crate) struct StartArgs {
    /// Video alias or canonical provider/model route
    #[arg(long, default_value = VIDEO_ALIAS)]
    model: String,
    /// What to render
    #[arg(long)]
    prompt: String,
    /// Provider size string, such as 1280x720
    #[arg(long)]
    size: Option<String>,
    /// Clip length in seconds
    #[arg(long)]
    seconds: Option<u32>,
    /// Print the provider's job document as JSON
    #[arg(long, default_value_t = false)]
    json: bool,
    /// Acknowledge that this command performs a billable provider request
    #[arg(long, default_value_t = false)]
    allow_provider_cost: bool,
}

#[derive(Args)]
pub(crate) struct StatusArgs {
    /// The provider's job identifier
    job_id: String,
    /// The same model the job was started with
    #[arg(long, default_value = VIDEO_ALIAS)]
    model: String,
    /// Print the provider's job document as JSON
    #[arg(long, default_value_t = false)]
    json: bool,
}

pub(crate) async fn video(command: VideoCommand) {
    match command {
        VideoCommand::Start(args) => start(args).await,
        VideoCommand::Status(args) => status(args).await,
    }
}

async fn start(args: StartArgs) {
    let StartArgs {
        model,
        prompt,
        size,
        seconds,
        json: as_json,
        allow_provider_cost,
    } = args;
    if !allow_provider_cost {
        eprintln!("refusing a billable video job without explicit --allow-provider-cost");
        std::process::exit(1);
    }
    let route = resolve_media_route(&model);
    let mut payload = Map::new();
    payload.insert("prompt".to_string(), Value::String(prompt));
    if let Some(size) = size {
        payload.insert("size".to_string(), Value::String(size));
    }
    if let Some(seconds) = seconds {
        payload.insert("seconds".to_string(), Value::String(seconds.to_string()));
    }
    match dispatch_direct_video(&route, payload).await {
        Ok(body) => answered(&model, &route, &body, as_json),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}

async fn status(args: StatusArgs) {
    let StatusArgs {
        job_id,
        model,
        json: as_json,
    } = args;
    let route = resolve_media_route(&model);
    match dispatch_direct_video_status(&route, &job_id).await {
        Ok(body) => answered(&model, &route, &body, as_json),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}

fn answered(model: &str, route: &str, body: &Value, as_json: bool) {
    if as_json {
        crate::cli::print_json(&job_json(model, route, body));
        return;
    }
    job_lines(model, route, body);
}
