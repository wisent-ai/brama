//! `brama music` and `brama voices`: songs, and the voice library behind
//! `voice-model`, from an operator shell. They run the same dispatch as
//! `POST /v1/audio/music` and `GET|POST /v1/audio/voices` and
//! `DELETE /v1/audio/voices/{voice_id}`.

use clap::{Args, Subcommand};
use serde_json::{Map, Value};

use brama::core::server::VOICE_ALIAS;
use brama::providers::adapter::VoiceSample;
use brama::subscription_dispatch::{
    dispatch_direct_music, dispatch_direct_voice_clone, dispatch_direct_voice_delete,
    dispatch_direct_voices,
};

use super::resolve_media_route;

#[derive(Args)]
pub(crate) struct MusicArgs {
    /// Canonical provider/model route that composes music, such as minimax/music-2.6
    #[arg(long)]
    model: String,
    /// The song's words, lines separated by newlines
    #[arg(long)]
    lyrics: String,
    /// Style, mood and instrumentation
    #[arg(long)]
    prompt: Option<String>,
    /// Audio container, such as mp3 or wav
    #[arg(long)]
    response_format: Option<String>,
    /// Where to write the song
    #[arg(long)]
    output: String,
    /// Acknowledge that this command performs a billable provider request
    #[arg(long, default_value_t = false)]
    allow_provider_cost: bool,
}

#[derive(Subcommand)]
pub(crate) enum VoicesCommand {
    /// List the voices the route's account can speak with
    List {
        /// Voice alias or canonical provider/model route
        #[arg(long, default_value = VOICE_ALIAS)]
        model: String,
    },
    /// Clone a voice from recordings
    Clone {
        /// Voice alias or canonical provider/model route
        #[arg(long, default_value = VOICE_ALIAS)]
        model: String,
        /// The new voice's name
        #[arg(long)]
        name: String,
        /// What the voice is
        #[arg(long)]
        description: Option<String>,
        /// A recording to clone from; repeat for several
        #[arg(long = "sample", required = true)]
        samples: Vec<String>,
        /// The recordings' content type, such as audio/mpeg
        #[arg(long)]
        content_type: String,
        /// Acknowledge that this command performs a billable provider request
        #[arg(long, default_value_t = false)]
        allow_provider_cost: bool,
    },
    /// Delete one voice from the route's account; the provider no longer
    /// keeps it, so this cannot be undone
    Remove {
        /// Voice alias or canonical provider/model route
        #[arg(long, default_value = VOICE_ALIAS)]
        model: String,
        /// The voice to delete, as `voices list` and `voices clone` print it
        voice_id: String,
        /// Confirm the deletion; the provider keeps no copy
        #[arg(long, default_value_t = false)]
        confirm: bool,
    },
}

pub(crate) async fn music(args: MusicArgs) {
    if !args.allow_provider_cost {
        eprintln!("refusing a billable music request without explicit --allow-provider-cost");
        std::process::exit(1);
    }
    let route = resolve_media_route(&args.model);
    let mut payload = Map::new();
    payload.insert("lyrics".to_string(), Value::String(args.lyrics));
    if let Some(prompt) = args.prompt {
        payload.insert("prompt".to_string(), Value::String(prompt));
    }
    if let Some(format) = args.response_format {
        payload.insert("response_format".to_string(), Value::String(format));
    }
    let song = dispatch_direct_music(&route, payload)
        .await
        .unwrap_or_else(|refused| {
            eprintln!("{refused}");
            std::process::exit(1);
        });
    if let Err(error) = std::fs::write(&args.output, &song.bytes) {
        eprintln!("cannot write {}: {error}", args.output);
        std::process::exit(1);
    }
    println!("Route: {route}");
    println!("Audio: {}", song.content_type);
    println!("Wrote {} bytes to {}", song.bytes.len(), args.output);
}

pub(crate) async fn voices(command: VoicesCommand) {
    match command {
        VoicesCommand::List { model } => {
            let route = resolve_media_route(&model);
            let body = dispatch_direct_voices(&route)
                .await
                .unwrap_or_else(|refused| {
                    eprintln!("{refused}");
                    std::process::exit(1);
                });
            super::super::print_json(&body);
        }
        VoicesCommand::Clone {
            model,
            name,
            description,
            samples,
            content_type,
            allow_provider_cost,
        } => {
            if !allow_provider_cost {
                eprintln!("refusing a billable voice clone without explicit --allow-provider-cost");
                std::process::exit(1);
            }
            let route = resolve_media_route(&model);
            let mut recordings = Vec::with_capacity(samples.len());
            for path in samples {
                let bytes = std::fs::read(&path).unwrap_or_else(|error| {
                    eprintln!("cannot read {path}: {error}");
                    std::process::exit(1);
                });
                let filename = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| {
                        eprintln!("{path} names no file");
                        std::process::exit(1);
                    });
                recordings.push(VoiceSample {
                    filename,
                    content_type: content_type.clone(),
                    bytes,
                });
            }
            let body =
                dispatch_direct_voice_clone(&route, &name, description.as_deref(), recordings)
                    .await
                    .unwrap_or_else(|refused| {
                        eprintln!("{refused}");
                        std::process::exit(1);
                    });
            super::super::print_json(&body);
        }
        VoicesCommand::Remove {
            model,
            voice_id,
            confirm,
        } => {
            if !confirm {
                eprintln!(
                    "refusing to delete voice {voice_id} without --confirm: the provider keeps no copy"
                );
                std::process::exit(2);
            }
            let route = resolve_media_route(&model);
            let body = dispatch_direct_voice_delete(&route, &voice_id)
                .await
                .unwrap_or_else(|refused| {
                    eprintln!("{refused}");
                    std::process::exit(1);
                });
            super::super::print_json(&body);
        }
    }
}
