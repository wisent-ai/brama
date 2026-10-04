//! Image, video, voice and music generation: the provider shapes that are
//! not chat and not a typed answer.
//!
//! An image is one request and one answer, so it reads like the typed
//! capabilities beside it. Video is a job: the create call returns an
//! identifier and a status, the render finishes minutes later, and the caller
//! reads it back. Brama keeps no record of that job — it holds no state a
//! restart would lose and no second source of truth about somebody else's
//! queue — so the caller keeps the identifier and names the same route when
//! it asks again. Voice and music answer audio rather than JSON ([`audio`]).
//!
//! A provider whose media contract is not OpenAI-shaped is spoken by its own
//! adapter ([`elevenlabs`], [`minimax`], [`gemini`]), chosen by the
//! declaration's `media_wire`.

mod audio;
mod edit;
mod elevenlabs;
mod form;
mod gemini;
mod minimax;

use serde_json::{Map, Value};

use super::super::registry::{endpoint, supports_image_route, supports_video_route, MediaWire};
use super::credential::authorize_provider;
use super::outcome::refusal::{provider_refused, transport_refusal};
use super::outcome::response_body::response_text;
use super::outcome::typed::{typed_object, typed_route, typed_transport};
use crate::types::{GatewayRefusal, Refusal};

pub use audio::{
    dispatch_music, dispatch_speech, dispatch_voice_clone, dispatch_voice_delete, dispatch_voices,
    SpokenAudio, VoiceSample,
};

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
    let (descriptor, model_id) = typed_route(route_id)?;
    // Gemini reads input images in its generation request, and so does a
    // provider that declares no edit path; one that declares an edit path
    // edits on it, because its generation endpoint takes no input image.
    let edits = !descriptor.image_edit_path.is_empty() && edit::asks_for_edit(&payload);
    if descriptor.media_wire == MediaWire::Gemini || edits {
        let (key, base_url, client) = typed_transport(descriptor, item, secret)?;
        let call = audio::Call {
            route_id,
            descriptor,
            model_id: &model_id,
            key: &key,
            base_url: &base_url,
            client: &client,
            secret,
        };
        if edits {
            return edit::edit_image(&call, payload).await;
        }
        return gemini::generate_image(&call, payload).await;
    }
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
    if !valid_path_segment(job_id) {
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
    let (status, _plan, text) = response_text(response).await?;
    if !status.is_success() {
        return Err(provider_refused(route_id, status, &text));
    }
    let mut body = typed_object(&text)?;
    if let Some(object) = body.as_object_mut() {
        object.insert("model".to_string(), Value::String(route_id.to_string()));
    }
    Ok(body)
}

/// Whether a value may be written into one URL path segment: a provider job
/// id or a voice id. Only characters that cannot leave the segment it was put
/// in are accepted.
fn valid_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}
