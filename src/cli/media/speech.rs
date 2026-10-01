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
    /// The provider's voice: a name such as alloy, or a voice id
    #[arg(long)]
    voice: String,
    /// Audio container the provider should encode, such as mp3 or wav
    #[arg(long)]
    response_format: Option<String>,
    /// Playback speed between 0.25 and 4.0
    #[arg(long)]
    speed: Option<f32>,
    /// ElevenLabs voice stability, 0.0 to 1.0
    #[arg(long)]
    stability: Option<f32>,
    /// ElevenLabs similarity boost, 0.0 to 1.0
    #[arg(long)]
    similarity_boost: Option<f32>,
    /// MiniMax delivery, such as happy or calm
    #[arg(long)]
    emotion: Option<String>,
    /// Where to write the spoken audio
    #[arg(long)]
    output: String,
    /// Acknowledge that this command performs a billable provider request
    #[arg(long, default_value_t = false)]
    allow_provider_cost: bool,
    /// Print the result as JSON instead of lines
    #[arg(long, default_value_t = false)]
    json: bool,
}

pub(crate) async fn speak(args: SpeakArgs) {
    let SpeakArgs {
        model,
        input,
        voice,
        response_format,
        speed,
        stability,
        similarity_boost,
        emotion,
        output,
        allow_provider_cost,
        json,
    } = args;
    if !allow_provider_cost {
        eprintln!("refusing a billable speech request without explicit --allow-provider-cost");
        std::process::exit(2);
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
    for (key, value) in [
        ("stability", stability),
        ("similarity_boost", similarity_boost),
    ] {
        if let Some(value) = value {
            payload.insert(
                key.to_string(),
                serde_json::Number::from_f64(f64::from(value)).map_or(Value::Null, Value::Number),
            );
        }
    }
    if let Some(emotion) = emotion {
        payload.insert("emotion".to_string(), Value::String(emotion));
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
    let written = serde_json::json!({
        "model": model,
        "route": route,
        "audio": spoken.content_type,
        "bytes": spoken.bytes.len(),
        "output": output,
    });
    super::super::print_answer(&written, json);
}
