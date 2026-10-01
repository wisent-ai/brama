//! The shapes whose answer is audio, and the voice library beside them.
//!
//! Speech and music are read whole rather than streamed: a spoken answer is
//! one artifact, the caller stores or plays it, and a partial file is worse
//! than a refusal. What comes back is what the provider sent, except where
//! the vendor wraps the audio in JSON and the adapter unwraps it.

use serde_json::{Map, Value};

use super::super::super::registry::{
    endpoint, supports_music_route, supports_speech_route, supports_voices_route, MediaWire,
    ProviderDescriptor,
};
use super::super::credential::authorize_provider;
use super::super::outcome::refusal::{provider_refused, transport_refusal};
use super::super::outcome::typed::{typed_route, typed_transport};
use super::{elevenlabs, minimax};
use crate::types::{GatewayRefusal, ProviderRefusal, Refusal};

/// A few minutes of speech or one song is a few megabytes; a body larger
/// than this is a provider malfunction rather than a sound.
const MAX_AUDIO_BYTES: usize = 24 * 1024 * 1024;

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
        MediaWire::OpenAi | MediaWire::Gemini => &[ELEVENLABS_ONLY, MINIMAX_ONLY],
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
    let call = Call {
        route_id,
        descriptor,
        model_id: &model_id,
        key: &key,
        base_url: &base_url,
        client: &client,
        secret,
    };
    match descriptor.media_wire {
        MediaWire::ElevenLabs => return elevenlabs::speak(&call, payload).await,
        MediaWire::MiniMax => return minimax::speak(&call, payload).await,
        MediaWire::OpenAi | MediaWire::Gemini => {}
    }
    payload.insert("model".to_string(), Value::String(model_id.clone()));
    let response = call
        .post(descriptor.speech_path)
        .json(&Value::Object(payload))
        .send()
        .await
        .map_err(|error| transport_refusal(&error))?;
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
    let call = Call {
        route_id,
        descriptor,
        model_id: &model_id,
        key: &key,
        base_url: &base_url,
        client: &client,
        secret,
    };
    match descriptor.media_wire {
        MediaWire::MiniMax => minimax::compose(&call, payload).await,
        MediaWire::OpenAi | MediaWire::ElevenLabs | MediaWire::Gemini => {
            Err(no_adapter(route_id, "music"))
        }
    }
}

/// The voices the account behind this route can speak with.
pub async fn dispatch_voices(route_id: &str, item: &str, secret: &str) -> Result<Value, Refusal> {
    let (descriptor, model_id) = voices_route(route_id)?;
    let (key, base_url, client) = typed_transport(descriptor, item, secret)?;
    let call = Call {
        route_id,
        descriptor,
        model_id: &model_id,
        key: &key,
        base_url: &base_url,
        client: &client,
        secret,
    };
    match descriptor.media_wire {
        MediaWire::ElevenLabs => elevenlabs::voices(&call).await,
        MediaWire::OpenAi | MediaWire::MiniMax | MediaWire::Gemini => {
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
    let call = Call {
        route_id,
        descriptor,
        model_id: &model_id,
        key: &key,
        base_url: &base_url,
        client: &client,
        secret,
    };
    match descriptor.media_wire {
        MediaWire::ElevenLabs => elevenlabs::clone_voice(&call, name, description, samples).await,
        MediaWire::OpenAi | MediaWire::MiniMax | MediaWire::Gemini => {
            Err(no_adapter(route_id, "voice library"))
        }
    }
}

/// Everything one vendor adapter needs to make its call.
pub(super) struct Call<'a> {
    pub(super) route_id: &'a str,
    pub(super) descriptor: &'static ProviderDescriptor,
    pub(super) model_id: &'a str,
    pub(super) key: &'a str,
    pub(super) base_url: &'a str,
    pub(super) client: &'a reqwest::Client,
    pub(super) secret: &'a str,
}

impl Call<'_> {
    pub(super) fn post(&self, path: &str) -> reqwest::RequestBuilder {
        authorize_provider(
            self.client.post(endpoint(self.base_url, path)),
            self.descriptor,
            self.key,
            self.secret,
        )
    }

    pub(super) fn get(&self, path: &str) -> reqwest::RequestBuilder {
        authorize_provider(
            self.client.get(endpoint(self.base_url, path)),
            self.descriptor,
            self.key,
            self.secret,
        )
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
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    if !status.is_success() {
        // A refused audio request answers JSON, not audio, so the ordinary
        // provider refusal reader applies to exactly this branch.
        let text = response.text().await.unwrap_or_default();
        return Err(provider_refused(route_id, status, &text));
    }
    let bytes = response.bytes().await.map_err(|error| {
        Refusal::new(
            ProviderRefusal::ProviderFailure,
            format!("provider_failure: audio was not delivered: {error}"),
        )
    })?;
    audio(content_type, bytes.to_vec())
}

/// Audio held to the size and non-emptiness every audio answer shares.
pub(super) fn audio(content_type: String, bytes: Vec<u8>) -> Result<SpokenAudio, Refusal> {
    if bytes.len() > MAX_AUDIO_BYTES {
        return Err(Refusal::new(
            ProviderRefusal::ProviderFailure,
            "provider_failure: audio exceeds the accepted size",
        ));
    }
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
