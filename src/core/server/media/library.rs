//! Music, and the voice library behind `voice-model`.
//!
//! `POST /v1/audio/music` composes one song from lyrics and answers the
//! audio. `GET /v1/audio/voices?model=…` lists the voices the deployment's
//! account on that route can speak with, and `POST /v1/audio/voices` clones
//! one from recordings. Each names `voice-model` or a canonical route whose
//! provider has that shape, exactly as the speech endpoint does.

use axum::extract::{Extension, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::requests::video_status_model;
use super::{api_error_for_alias, dispatched, refused_typed};
use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::aliases::VOICE_ALIAS;
use crate::core::server::refusal::{api_error, ApiError};
use crate::core::server::telemetry::record_typed_request;
use crate::providers::adapter::{supports_music_route, supports_voices_route, VoiceSample};
use crate::subscription_dispatch::{
    dispatch_direct_music, dispatch_direct_voice_clone, dispatch_direct_voices,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct MusicRequest {
    model: String,
    lyrics: String,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    response_format: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::core::server) struct VoiceCloneRequest {
    model: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    samples: Vec<Sample>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sample {
    data_base64: String,
    content_type: String,
    filename: String,
}

/// Compose one song. The answer is the provider's audio, as the speech
/// endpoint answers.
pub(in crate::core::server) async fn audio_music(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<MusicRequest>,
) -> Result<Response, ApiError> {
    if request.lyrics.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "lyrics must carry the song's words",
        ));
    }
    let route = library_route(
        &client_identity,
        &aliases,
        &request.model,
        supports_music_route,
        "compose music",
    )?;
    let mut payload = Map::new();
    payload.insert("lyrics".to_string(), Value::String(request.lyrics));
    if let Some(prompt) = request.prompt.filter(|prompt| !prompt.trim().is_empty()) {
        payload.insert("prompt".to_string(), Value::String(prompt));
    }
    if let Some(format) = request
        .response_format
        .filter(|format| !format.trim().is_empty())
    {
        payload.insert("response_format".to_string(), Value::String(format));
    }
    let song = dispatch_direct_music(&route, payload)
        .await
        .map_err(|refused| refused_typed(&refused))?;
    record_typed_request(u32::from(true), false);
    Ok((
        [
            (header::CONTENT_TYPE, song.content_type),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        song.bytes,
    )
        .into_response())
}

/// The voices one route's account can speak with.
pub(in crate::core::server) async fn audio_voices(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let model = video_status_model(query).map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "a voice list names the route whose account it reads: GET /v1/audio/voices?model=…",
        )
    })?;
    let route = library_route(
        &client_identity,
        &aliases,
        &model,
        supports_voices_route,
        "list voices",
    )?;
    let body = dispatched(dispatch_direct_voices(&route).await)?;
    record_typed_request(u32::from(true), false);
    Ok(Json(body))
}

/// Clone one voice from recordings; the answer carries the new `voice_id`.
pub(in crate::core::server) async fn audio_voice_clone(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<VoiceCloneRequest>,
) -> Result<Json<Value>, ApiError> {
    if request.name.trim().is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "name must name the voice",
        ));
    }
    if request.samples.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "samples must carry at least one recording",
        ));
    }
    let route = library_route(
        &client_identity,
        &aliases,
        &request.model,
        supports_voices_route,
        "clone voices",
    )?;
    let mut samples = Vec::with_capacity(request.samples.len());
    for (index, sample) in request.samples.into_iter().enumerate() {
        let bytes = BASE64.decode(sample.data_base64.as_bytes()).map_err(|_| {
            api_error(
                StatusCode::BAD_REQUEST,
                &format!("samples[{index}].data_base64 is not base64"),
            )
        })?;
        if bytes.is_empty() {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                &format!("samples[{index}] holds no audio"),
            ));
        }
        samples.push(VoiceSample {
            filename: sample.filename,
            content_type: sample.content_type,
            bytes,
        });
    }
    let body = dispatched(
        dispatch_direct_voice_clone(
            &route,
            &request.name,
            request.description.as_deref(),
            samples,
        )
        .await,
    )?;
    record_typed_request(u32::from(true), false);
    Ok(Json(body))
}

/// The route one library request runs on: `voice-model`, resolved to the
/// deployment's voice route, or a canonical route the caller's bearer
/// allows. Either must reach a provider with this shape.
fn library_route(
    client_identity: &ModelClientIdentity,
    aliases: &ModelAliases,
    requested: &str,
    supported: fn(&str) -> bool,
    produces: &str,
) -> Result<String, ApiError> {
    if !client_identity.authorizes_model(requested) {
        return Err(api_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    let route = if requested == VOICE_ALIAS {
        aliases.voice_route(requested).ok_or_else(|| {
            let diagnosis = aliases.diagnose(requested);
            api_error_for_alias(requested, diagnosis.reason)
        })?
    } else {
        requested.to_string()
    };
    if supported(&route) {
        return Ok(route);
    }
    Err(api_error(
        StatusCode::BAD_REQUEST,
        &format!(
            "`{route}` cannot {produces}: name `{VOICE_ALIAS}` or a provider/model route whose provider does"
        ),
    ))
}
