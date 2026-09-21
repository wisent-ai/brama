//! `brama speak`: one spoken answer from an operator shell.
//!
//! The provider returns encoded audio rather than JSON, so this command
//! writes a file and says how many bytes went where. Printing audio to a
//! terminal is not an outcome anybody wanted, which is why `--output` is
//! required rather than optional.

use clap::Args;
use serde_json::{Map, Value};

use brama::core::server::VOICE_ALIAS;
use brama::subscription_dispatch::dispatch_direct_speech;

use super::resolve_media_route;

#[derive(Args)]
pub(crate) struct SpeakArgs {
    /// Voice alias or canonical provider/model route
    #[arg(long, default_value = VOICE_ALIAS)]
    model: String,
    /// The text to speak
    #[arg(long)]
    input: String,
    /// The provider's voice name, such as alloy
    #[arg(long)]
    voice: String,
    /// Audio container the provider should encode, such as mp3 or wav
    #[arg(long)]
    response_format: Option<String>,
    /// Playback speed between 0.25 and 4.0
    #[arg(long)]
    speed: Option<f32>,
    /// Where to write the spoken audio
    #[arg(long)]
    output: String,
    /// Acknowledge that this command performs a billable provider request
    #[arg(long, default_value_t = false)]
    allow_provider_cost: bool,
}

pub(crate) async fn speak(args: SpeakArgs) {
    let SpeakArgs {
        model,
        input,
        voice,
        response_format,
        speed,
        output,
        allow_provider_cost,
    } = args;
    if !allow_provider_cost {
        eprintln!("refusing a billable speech request without explicit --allow-provider-cost");
        std::process::exit(1);
    }
    let route = resolve_media_route(&model);
    let mut payload = Map::new();
    payload.insert("input".to_string(), Value::String(input));
    payload.insert("voice".to_string(), Value::String(voice));
    if let Some(format) = response_format {
        payload.insert("response_format".to_string(), Value::String(format));
    }
    if let Some(speed) = speed {
        payload.insert(
            "speed".to_string(),
            serde_json::Number::from_f64(f64::from(speed)).map_or(Value::Null, Value::Number),
        );
    }
    let spoken = match dispatch_direct_speech(&route, payload).await {
        Ok(spoken) => spoken,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };
    if let Err(error) = std::fs::write(&output, &spoken.bytes) {
        eprintln!("cannot write {output}: {error}");
        std::process::exit(1);
    }
    println!("Model: {model}");
    println!("Route: {route}");
    println!("Audio: {}", spoken.content_type);
    println!("Wrote {} bytes to {output}", spoken.bytes.len());
}
