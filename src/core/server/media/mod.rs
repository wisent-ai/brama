//! The three generation shapes that are not text: `POST
//! /v1/images/generations`, `POST /v1/videos` with `GET /v1/videos/{id}`
//! reading one started job back, and `POST /v1/audio/speech`.
//!
//! Both accept the same two kinds of name the chat endpoints do — the
//! deployment's media alias, or a canonical `provider/model` route the
//! caller's bearer allows — and neither accepts the other's. A chat model
//! named here is refused by name rather than posted to an endpoint its
//! provider does not serve, and the same holds in reverse on the chat path.
//!
//! Media is paid by this deployment's own provider key. No subscription in
//! the pool carries an image or video quota, so there is nothing for `best`
//! to delegate to and no caller plan to bill.

mod requests;

use axum::extract::{Extension, Path, Query};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::aliases::{IMAGE_ALIAS, VIDEO_ALIAS, VOICE_ALIAS};
use crate::core::server::refusal::contract::model_error_contract;
use crate::core::server::refusal::envelope::{typed_dispatch_attempts, typed_dispatch_error};
use crate::core::server::refusal::{api_error, ApiError};
use crate::core::server::telemetry::record_typed_request;
use crate::providers::adapter::{
    supports_image_route, supports_speech_route, supports_video_route, ModelKind,
};
use crate::subscription_dispatch::{
    dispatch_direct_image, dispatch_direct_speech, dispatch_direct_video,
    dispatch_direct_video_status,
};

use requests::{video_status_model, ImageRequest, SpeechRequest, VideoRequest};

pub(in crate::core::server) async fn image_generations(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<ImageRequest>,
) -> Result<Json<Value>, ApiError> {
    if let Some(reason) = request.invalid() {
        return Err(api_error(StatusCode::BAD_REQUEST, reason));
    }
    let route = media_route(&client_identity, &aliases, &request.model, Shape::Image).await?;
    let body = dispatched(dispatch_direct_image(&route, request.payload()).await)?;
    if !body.get("data").is_some_and(Value::is_array) {
        record_typed_request(u32::from(true), true);
        return Err(typed_dispatch_error("image provider returned no images"));
    }
    record_typed_request(u32::from(true), false);
    Ok(Json(body))
}

/// Start one video job. The answer is the provider's job — an id and a status
/// — not a render: the caller reads it back from the status route with the
/// same model name, because this gateway stores nothing about the job.
pub(in crate::core::server) async fn video_generations(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<VideoRequest>,
) -> Result<Json<Value>, ApiError> {
    if let Some(reason) = request.invalid() {
        return Err(api_error(StatusCode::BAD_REQUEST, reason));
    }
    let route = media_route(&client_identity, &aliases, &request.model, Shape::Video).await?;
    let body = dispatched(dispatch_direct_video(&route, request.payload()).await)?;
    if !body.get("id").is_some_and(Value::is_string) {
        record_typed_request(u32::from(true), true);
        return Err(typed_dispatch_error("video provider returned no job id"));
    }
    record_typed_request(u32::from(true), false);
    Ok(Json(body))
}

pub(in crate::core::server) async fn video_status(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Path(video_id): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let model =
        video_status_model(query).map_err(|reason| api_error(StatusCode::BAD_REQUEST, reason))?;
    let route = media_route(&client_identity, &aliases, &model, Shape::Video).await?;
    let body = dispatched(dispatch_direct_video_status(&route, &video_id).await)?;
    record_typed_request(u32::from(true), false);
    Ok(Json(body))
}

/// Speak one text. The answer is the provider's audio, handed back with the
/// content type it stated: this endpoint returns bytes rather than JSON,
/// because a caller that has to decode base64 out of an envelope to play a
/// sound is doing the gateway's work.
pub(in crate::core::server) async fn audio_speech(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<SpeechRequest>,
) -> Result<Response, ApiError> {
    if let Some(reason) = request.invalid() {
        return Err(api_error(StatusCode::BAD_REQUEST, reason));
    }
    let route = media_route(&client_identity, &aliases, &request.model, Shape::Voice).await?;
    let spoken = dispatch_direct_speech(&route, request.payload())
        .await
        .map_err(|message| {
            let attempts = typed_dispatch_attempts(model_error_contract(&message));
            record_typed_request(attempts, true);
            typed_dispatch_error(&message)
        })?;
    record_typed_request(u32::from(true), false);
    Ok((
        [
            (header::CONTENT_TYPE, spoken.content_type),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
        spoken.bytes,
    )
        .into_response())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Image,
    Video,
    Voice,
}

impl Shape {
    fn alias(self) -> &'static str {
        match self {
            Shape::Image => IMAGE_ALIAS,
            Shape::Video => VIDEO_ALIAS,
            Shape::Voice => VOICE_ALIAS,
        }
    }

    fn supported(self, route: &str) -> bool {
        match self {
            Shape::Image => supports_image_route(route),
            Shape::Video => supports_video_route(route),
            Shape::Voice => supports_speech_route(route),
        }
    }

    fn kind(self) -> ModelKind {
        match self {
            Shape::Image => ModelKind::Image,
            Shape::Video => ModelKind::Video,
            Shape::Voice => ModelKind::Audio,
        }
    }

    fn produces(self) -> &'static str {
        match self {
            Shape::Image => "generate images",
            Shape::Video => "generate video",
            Shape::Voice => "generate speech",
        }
    }
}

/// The route one media request runs on: the deployment's media alias, or a
/// canonical route the caller's bearer allows, a declared provider can reach
/// and the catalogue lists as this kind.
///
/// The provider check alone was not enough, and the gap had teeth: OpenAI
/// declares an image path, so `openai/gpt-4o-mini` passed it and the caller
/// learned its mistake from a redeemed credential and a vendor error. The
/// catalogue already says what each model produces, so the kind decides
/// here, before anything is paid for. A route the catalogue does not carry
/// is still allowed: a provider serves models no public list knows, and
/// refusing those would make the catalogue's blind spots into Brama's.
async fn media_route(
    client_identity: &ModelClientIdentity,
    aliases: &ModelAliases,
    requested: &str,
    shape: Shape,
) -> Result<String, ApiError> {
    if !client_identity.authorizes_model(requested) {
        return Err(api_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    if requested == shape.alias() {
        let resolved = match shape {
            Shape::Image => aliases.image_route(requested),
            Shape::Video => aliases.video_route(requested),
            Shape::Voice => aliases.voice_route(requested),
        };
        return resolved.ok_or_else(|| {
            let diagnosis = aliases.diagnose(requested);
            api_error_for_alias(requested, diagnosis.reason)
        });
    }
    if shape.supported(requested) {
        if let Some(listed) = catalogue_kind(requested).await {
            if listed != shape.kind() {
                return Err(api_error(
                    StatusCode::BAD_REQUEST,
                    &format!(
                        "`{requested}` is a {} model: name `{}` or a provider/model route the catalogue lists as {}",
                        listed.as_str(),
                        shape.alias(),
                        shape.kind().as_str()
                    ),
                ));
            }
        }
        return Ok(requested.to_string());
    }
    Err(api_error(
        StatusCode::BAD_REQUEST,
        &format!(
            "`{requested}` cannot {}: name `{}` or a provider/model route whose provider does",
            shape.produces(),
            shape.alias()
        ),
    ))
}

/// What the public catalogue says this route produces, when it carries it.
async fn catalogue_kind(route_id: &str) -> Option<ModelKind> {
    let snapshot = crate::subscription_dispatch::model_catalog::snapshot()
        .await
        .ok()?;
    snapshot
        .models
        .iter()
        .find(|model| model.route_id == route_id)
        .map(|model| model.kind())
}

/// An alias this gateway knows and cannot serve is a configuration fault on
/// this host, so it answers `503` with the repair, exactly as the chat path
/// does — not `400`, which tells the caller its own request was wrong.
fn api_error_for_alias(alias: &str, reason: Option<String>) -> ApiError {
    api_error(
        StatusCode::SERVICE_UNAVAILABLE,
        &reason.unwrap_or_else(|| format!("alias `{alias}` is not declared on this gateway")),
    )
}

fn dispatched(result: Result<Value, String>) -> Result<Value, ApiError> {
    result.map_err(|message| {
        let attempts = typed_dispatch_attempts(model_error_contract(&message));
        record_typed_request(attempts, true);
        typed_dispatch_error(&message)
    })
}
