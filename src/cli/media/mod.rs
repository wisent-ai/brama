//! `brama image`, `brama video` and `brama speak`: the three generation
//! shapes that are not text, from an operator shell.
//!
//! They exist for the same two moments `brama decide` does: establishing that
//! a media route answers at all before a gateway is running, and diagnosing
//! one that stopped. Each runs the same dispatch the HTTP endpoint runs, so
//! the refusal printed here is the sentence a caller would have received.
//!
//! Every one of them spends money at a provider, so each requires
//! `--allow-provider-cost` before it sends anything, exactly as `brama
//! decide` does.

mod artifact;
mod speech;
mod video;

use clap::Args;
use serde_json::{json, Map, Value};

use brama::core::server::{alias_routing, IMAGE_ALIAS};
use brama::subscription_dispatch::dispatch_direct_image;

pub(crate) use speech::{speak, SpeakArgs};
pub(crate) use video::{video, VideoCommand};

use artifact::write_artifact;

#[derive(Args)]
pub(crate) struct ImageArgs {
    /// Image alias or canonical provider/model route
    #[arg(long, default_value = IMAGE_ALIAS)]
    model: String,
    /// What to render
    #[arg(long)]
    prompt: String,
    /// Provider size string, such as 1024x1024
    #[arg(long)]
    size: Option<String>,
    /// How many images to render
    #[arg(long)]
    n: Option<u32>,
    /// Provider quality string
    #[arg(long)]
    quality: Option<String>,
    /// Write the rendered bytes to this file
    #[arg(long)]
    output: Option<String>,
    /// Print the provider's answer as JSON
    #[arg(long, default_value_t = false)]
    json: bool,
    /// Acknowledge that this command performs a billable provider request
    #[arg(long, default_value_t = false)]
    allow_provider_cost: bool,
}

pub(crate) async fn image(args: ImageArgs) {
    let ImageArgs {
        model,
        prompt,
        size,
        n,
        quality,
        output,
        json: as_json,
        allow_provider_cost,
    } = args;
    if !allow_provider_cost {
        eprintln!("refusing a billable image generation without explicit --allow-provider-cost");
        std::process::exit(1);
    }
    let route = resolve_media_route(&model);
    let mut payload = Map::new();
    payload.insert("prompt".to_string(), Value::String(prompt));
    if let Some(count) = n {
        payload.insert("n".to_string(), Value::from(count));
    }
    if let Some(size) = size {
        payload.insert("size".to_string(), Value::String(size));
    }
    if let Some(quality) = quality {
        payload.insert("quality".to_string(), Value::String(quality));
    }
    let body = match dispatch_direct_image(&route, payload).await {
        Ok(body) => body,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };
    if as_json {
        super::print_json(&body);
        return;
    }
    println!("Model: {model}");
    println!("Route: {route}");
    let images = body
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    println!("Rendered {} image(s)", images.len());
    for (index, image) in images.iter().enumerate() {
        if let Some(url) = image.get("url").and_then(Value::as_str) {
            println!("[{index}] {url}");
        }
        let Some(encoded) = image.get("b64_json").and_then(Value::as_str) else {
            continue;
        };
        match output.as_deref() {
            Some(path) => write_artifact(path, index, images.len(), encoded),
            None => println!(
                "[{index}] {} base64 bytes, no --output given",
                encoded.len()
            ),
        }
    }
}

/// An alias resolves through the gateway's own alias table; anything else is
/// handed on as the canonical route it is, and the dispatch refuses it by
/// name when its provider renders nothing.
pub(super) fn resolve_media_route(model: &str) -> String {
    if model.contains('/') {
        return model.to_string();
    }
    match alias_routing(model) {
        Ok((Some(route), _)) => route,
        Ok((None, diagnosis)) => {
            eprintln!(
                "{}",
                diagnosis
                    .reason
                    .unwrap_or_else(|| format!("alias `{model}` has no serviceable route"))
            );
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("the alias table could not be read: {error}");
            std::process::exit(1);
        }
    }
}

/// The job document `brama video` prints, in one place so `start` and
/// `status` cannot describe the same provider answer differently.
pub(super) fn job_lines(model: &str, route: &str, body: &Value) {
    println!("Model: {model}");
    println!("Route: {route}");
    println!(
        "Job: {}",
        body.get("id").and_then(Value::as_str).unwrap_or("-")
    );
    println!(
        "Status: {}",
        body.get("status").and_then(Value::as_str).unwrap_or("-")
    );
    if let Some(progress) = body.get("progress") {
        println!("Progress: {progress}");
    }
    if let Some(error) = body.get("error").filter(|value| !value.is_null()) {
        println!("Error: {error}");
    }
}

/// The shape `--json` prints for a job, so a script reads the same fields the
/// HTTP surface answers.
pub(super) fn job_json(model: &str, route: &str, body: &Value) -> Value {
    json!({
        "model": model,
        "route": route,
        "job": body,
    })
}
