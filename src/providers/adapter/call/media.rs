//! Image, video and voice generation: the three provider shapes that are not
//! chat and not a typed answer.
//!
//! An image is one request and one answer, so it reads like the typed
//! capabilities beside it. Video is a job: the create call returns an
//! identifier and a status, the render finishes minutes later, and the caller
//! reads it back. Brama keeps no record of that job — it holds no state a
//! restart would lose and no second source of truth about somebody else's
//! queue — so the caller keeps the identifier and names the same route when
//! it asks again. Voice is the one shape whose answer is not JSON at all: the
//! provider returns encoded audio, and those bytes are handed back as they
//! arrived, with the content type the provider stated.

use serde_json::{Map, Value};

use super::super::registry::{
    endpoint, supports_image_route, supports_speech_route, supports_video_route,
};
use super::credential::authorize_provider;
use super::outcome::refusal::{provider_refused, transport_refusal};
use super::outcome::response_body::bounded_response_text;
use super::outcome::typed::{typed_object, typed_route, typed_transport};
use crate::types::{GatewayRefusal, ProviderRefusal, Refusal};

/// Audio is answered as bytes, so a ceiling belongs here rather than in the
/// JSON reader: sixty seconds of speech is a few hundred kilobytes, and a
/// body larger than this is a provider malfunction rather than a voice.
const MAX_SPEECH_BYTES: usize = 24 * 1024 * 1024;

/// One spoken answer: the encoded audio and the content type the provider
/// stated for it, which is what the caller needs to store or play the bytes.
pub struct SpokenAudio {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// Speak one text through a route whose provider declares a voice.
///
/// The bytes are read whole rather than streamed: a spoken answer is one
/// artifact, the caller stores or plays it, and a partial file is worse than
/// a refusal. What comes back is exactly what the provider sent — Brama
/// re-encodes nothing and inspects nothing beyond the size.
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
    let (key, base_url, client) = typed_transport(descriptor, item, secret)?;
    payload.insert("model".to_string(), Value::String(model_id));
    let response = authorize_provider(
        client.post(endpoint(&base_url, descriptor.speech_path)),
        descriptor,
        &key,
        secret,
    )
    .json(&Value::Object(payload))
    .send()
    .await
    .map_err(|error| transport_refusal(&error))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    if !status.is_success() {
        // A refused speech request answers JSON, not audio, so the ordinary
        // provider refusal reader applies to exactly this branch.
        let text = response.text().await.unwrap_or_default();
        return Err(provider_refused(route_id, status, &text));
    }
    let bytes = response.bytes().await.map_err(|error| {
        Refusal::new(
            ProviderRefusal::ProviderFailure,
            format!("provider_failure: spoken audio was not delivered: {error}"),
        )
    })?;
    if bytes.len() > MAX_SPEECH_BYTES {
        return Err(Refusal::new(
            ProviderRefusal::ProviderFailure,
            "provider_failure: spoken audio exceeds the accepted size",
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
        bytes: bytes.to_vec(),
    })
}

/// A provider's job identifier travels into a URL path, so it is held to what
/// an identifier can be: printable, short, and unable to leave the path
/// segment it was put in.
const MAX_JOB_ID_BYTES: usize = 128;

pub async fn dispatch_image(
    route_id: &str,
    payload: Map<String, Value>,
    item: &str,
    secret: &str,
) -> Result<Value, Refusal> {
    if !supports_image_route(route_id) {
        return Err(Refusal::gateway(
            GatewayRefusal::InvalidRequest,
            format!("invalid_request: route `{route_id}` does not generate images"),
        ));
    }
    let (descriptor, _) = typed_route(route_id)?;
    generate(route_id, descriptor.image_path, payload, item, secret).await
}

pub async fn dispatch_video(
    route_id: &str,
    payload: Map<String, Value>,
    item: &str,
    secret: &str,
) -> Result<Value, Refusal> {
    if !supports_video_route(route_id) {
        return Err(does_not_generate_video(route_id));
    }
    let (descriptor, _) = typed_route(route_id)?;
    generate(route_id, descriptor.video_path, payload, item, secret).await
}

fn does_not_generate_video(route_id: &str) -> Refusal {
    Refusal::gateway(
        GatewayRefusal::InvalidRequest,
        format!("invalid_request: route `{route_id}` does not generate video"),
    )
}

/// Read one started video job back from the provider that started it.
pub async fn dispatch_video_status(
    route_id: &str,
    job_id: &str,
    item: &str,
    secret: &str,
) -> Result<Value, Refusal> {
    if !supports_video_route(route_id) {
        return Err(does_not_generate_video(route_id));
    }
    if !valid_job_id(job_id) {
        return Err(Refusal::gateway(
            GatewayRefusal::InvalidRequest,
            "invalid_request: video id is not a provider job identifier",
        ));
    }
    let (descriptor, _) = typed_route(route_id)?;
    let (key, base_url, client) = typed_transport(descriptor, item, secret)?;
    let path = descriptor.video_status_path.replace("{id}", job_id);
    let response = authorize_provider(
        client.get(endpoint(&base_url, &path)),
        descriptor,
        &key,
        secret,
    )
    .send()
    .await
    .map_err(|error| transport_refusal(&error))?;
    answered(route_id, response).await
}

async fn generate(
    route_id: &str,
    path: &str,
    mut payload: Map<String, Value>,
    item: &str,
    secret: &str,
) -> Result<Value, Refusal> {
    let (descriptor, model_id) = typed_route(route_id)?;
    let (key, base_url, client) = typed_transport(descriptor, item, secret)?;
    payload.insert("model".to_string(), Value::String(model_id));
    let response = authorize_provider(
        client.post(endpoint(&base_url, path)),
        descriptor,
        &key,
        secret,
    )
    .json(&Value::Object(payload))
    .send()
    .await
    .map_err(|error| transport_refusal(&error))?;
    answered(route_id, response).await
}

/// One provider answer, refused or handed back with the route the caller
/// named written back into it. The caller asked for `openai/gpt-image-1`; the
/// provider answers with its own bare model id, and a body that disagreed
/// with the request would send the next call to a name Brama cannot route.
async fn answered(route_id: &str, response: reqwest::Response) -> Result<Value, Refusal> {
    let (status, _plan, text) = bounded_response_text(response).await?;
    if !status.is_success() {
        return Err(provider_refused(route_id, status, &text));
    }
    let mut body = typed_object(&text)?;
    if let Some(object) = body.as_object_mut() {
        object.insert("model".to_string(), Value::String(route_id.to_string()));
    }
    Ok(body)
}

fn valid_job_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_JOB_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}
