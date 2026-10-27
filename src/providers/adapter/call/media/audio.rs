//! The shapes whose answer is audio, and the voice library beside them.
//!
//! Speech and music are read whole rather than streamed: a spoken answer is
//! one artifact, the caller stores or plays it, and a partial file is worse
//! than a refusal. What comes back is what the provider sent, except where
//! the vendor wraps the audio in JSON and the adapter unwraps it.

use serde_json::{Map, Value};

use super::super::super::registry::{
    supports_music_route, supports_speech_route, supports_voices_route, MediaWire,
    ProviderDescriptor,
};
use super::super::outcome::refusal::provider_refused;
use super::super::outcome::response_body::response_text;
use super::super::outcome::typed::{typed_route, typed_transport};
use super::call::Call;
use super::{elevenlabs, minimax};
use crate::types::{GatewayRefusal, ProviderRefusal, Refusal};

/// The speech options only one vendor's contract carries. Sent anywhere else
/// they would be dropped or refused by the vendor, so they are refused here
/// by name.
const ELEVENLABS_ONLY: &[&str] = &["stability", "similarity_boost", "timestamps"];
const MINIMAX_ONLY: &[&str] = &["emotion"];

/// One spoken or sung answer: the encoded audio and the content type the
/// provider stated for it.
pub struct SpokenAudio {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// One recording a voice is cloned from.
pub struct VoiceSample {
    pub filename: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// Speak one text through a route whose provider declares a voice.
pub async fn dispatch_speech(
    route_id: &str,
    mut payload: Map<String, Value>,
    item: &str,
    secret: &str,
) -> Result<SpokenAudio, Refusal> {
    if !supports_speech_route(route_id) {
        return Err(Refusal::gateway(
            GatewayRefusal::InvalidRequest,
            format!("invalid_request: route `{route_id}` does not generate speech"),
        ));
    }
    let (descriptor, model_id) = typed_route(route_id)?;
    let foreign: &[&[&str]] = match descriptor.media_wire {
        MediaWire::ElevenLabs => &[MINIMAX_ONLY],
        MediaWire::MiniMax => &[ELEVENLABS_ONLY],
        MediaWire::OpenAi | MediaWire::Gemini | MediaWire::OpenRouter => {
            &[ELEVENLABS_ONLY, MINIMAX_ONLY]
        }
    };
    if let Some(option) = foreign
        .iter()
        .flat_map(|options| options.iter())
        .find(|option| payload.contains_key(**option))
    {
        return Err(Refusal::gateway(
            GatewayRefusal::InvalidRequest,
            format!("invalid_request: route `{route_id}` takes no `{option}` option"),
        ));
    }
    let (key, base_url, client) = typed_transport(descriptor, item, secret)?;
    let call = Call::new(
        route_id, descriptor, &model_id, &key, &base_url, &client, secret,
    )?;
    match descriptor.media_wire {
        MediaWire::ElevenLabs => return elevenlabs::speak(&call, payload).await,
        MediaWire::MiniMax => return minimax::speak(&call, payload).await,
        MediaWire::OpenAi | MediaWire::Gemini | MediaWire::OpenRouter => {}
    }
    payload.insert("model".to_string(), Value::String(model_id.clone()));
    let builder = call
        .post(descriptor.speech_path)
        .json(&Value::Object(payload));
    let response = call.send(builder, "send provider speech").await?;
    audio_answer(route_id, response).await
}

/// Compose one song from lyrics through a route whose provider declares
/// music.
pub async fn dispatch_music(
    route_id: &str,
    payload: Map<String, Value>,
    item: &str,
    secret: &str,
) -> Result<SpokenAudio, Refusal> {
    if !supports_music_route(route_id) {
        return Err(Refusal::gateway(
            GatewayRefusal::InvalidRequest,
            format!("invalid_request: route `{route_id}` does not compose music"),
        ));
    }
    let (descriptor, model_id) = typed_route(route_id)?;
    let (key, base_url, client) = typed_transport(descriptor, item, secret)?;
    let call = Call::new(
        route_id, descriptor, &model_id, &key, &base_url, &client, secret,
    )?;
    match descriptor.media_wire {
        MediaWire::MiniMax => minimax::compose(&call, payload).await,
        MediaWire::OpenAi | MediaWire::ElevenLabs | MediaWire::Gemini | MediaWire::OpenRouter => {
            Err(no_adapter(route_id, "music"))
        }
    }
}

/// The voices the account behind this route can speak with.
pub async fn dispatch_voices(route_id: &str, item: &str, secret: &str) -> Result<Value, Refusal> {
    let (descriptor, model_id) = voices_route(route_id)?;
    let (key, base_url, client) = typed_transport(descriptor, item, secret)?;
    let call = Call::new(
        route_id, descriptor, &model_id, &key, &base_url, &client, secret,
    )?;
    match descriptor.media_wire {
        MediaWire::ElevenLabs => elevenlabs::voices(&call).await,
        MediaWire::OpenAi | MediaWire::MiniMax | MediaWire::Gemini | MediaWire::OpenRouter => {
            Err(no_adapter(route_id, "voice library"))
        }
    }
}

/// Clone one voice from recordings on the account behind this route.
pub async fn dispatch_voice_clone(
    route_id: &str,
    name: &str,
    description: Option<&str>,
    samples: Vec<VoiceSample>,
    item: &str,
    secret: &str,
) -> Result<Value, Refusal> {
    let (descriptor, model_id) = voices_route(route_id)?;
    let (key, base_url, client) = typed_transport(descriptor, item, secret)?;
    let call = Call::new(
        route_id, descriptor, &model_id, &key, &base_url, &client, secret,
    )?;
    match descriptor.media_wire {
        MediaWire::ElevenLabs => elevenlabs::clone_voice(&call, name, description, samples).await,
        MediaWire::OpenAi | MediaWire::MiniMax | MediaWire::Gemini | MediaWire::OpenRouter => {
            Err(no_adapter(route_id, "voice library"))
        }
    }
}

/// Delete one voice from the account behind this route.
pub async fn dispatch_voice_delete(
    route_id: &str,
    voice_id: &str,
    item: &str,
    secret: &str,
) -> Result<Value, Refusal> {
    let (descriptor, model_id) = voices_route(route_id)?;
    let (key, base_url, client) = typed_transport(descriptor, item, secret)?;
    let call = Call::new(
        route_id, descriptor, &model_id, &key, &base_url, &client, secret,
    )?;
    match descriptor.media_wire {
        MediaWire::ElevenLabs => elevenlabs::delete_voice(&call, voice_id).await,
        MediaWire::OpenAi | MediaWire::MiniMax | MediaWire::Gemini | MediaWire::OpenRouter => {
            Err(no_adapter(route_id, "voice library"))
        }
    }
}

fn voices_route(route_id: &str) -> Result<(&'static ProviderDescriptor, String), Refusal> {
    if !supports_voices_route(route_id) {
        return Err(Refusal::gateway(
            GatewayRefusal::InvalidRequest,
            format!("invalid_request: route `{route_id}` has no voice library"),
        ));
    }
    typed_route(route_id)
}

fn no_adapter(route_id: &str, shape: &str) -> Refusal {
    Refusal::gateway(
        GatewayRefusal::InvalidRequest,
        format!("invalid_request: route `{route_id}` has no {shape} adapter"),
    )
}

/// A provider answer whose body is the audio itself.
pub(super) async fn audio_answer(
    route_id: &str,
    response: reqwest::Response,
) -> Result<SpokenAudio, Refusal> {
    let status = response.status();
    if !status.is_success() {
        // The provider refusal body is JSON, not audio.
        let (status, _plan, text) = response_text(response).await?;
        return Err(provider_refused(route_id, status, &text));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .ok_or_else(|| {
            Refusal::new(
                ProviderRefusal::ProviderFailure,
                "provider_failure: successful audio response has no Content-Type",
            )
        })?
        .to_str()
        .map_err(|error| {
            Refusal::new(
                ProviderRefusal::ProviderFailure,
                format!("provider_failure: audio Content-Type is not text: {error}"),
            )
        })?
        .to_string();
    let url = response.url().to_string();
    let bytes = stado_wait::until(
        stado_wait::Kind::Network,
        "read provider audio",
        &url,
        response.bytes(),
    )
    .await
    .map_err(|error| {
        Refusal::new(
            ProviderRefusal::ProviderFailure,
            format!("provider_failure: audio was not delivered: {error}"),
        )
    })?;
    audio(content_type, bytes.to_vec())
}

/// Audio held to the non-emptiness every audio answer shares.
pub(super) fn audio(content_type: String, bytes: Vec<u8>) -> Result<SpokenAudio, Refusal> {
    if bytes.is_empty() {
        return Err(Refusal::new(
            ProviderRefusal::ProviderFailure,
            "provider_failure: provider returned no audio",
        ));
    }
    Ok(SpokenAudio {
        content_type,
        bytes,
    })
}
