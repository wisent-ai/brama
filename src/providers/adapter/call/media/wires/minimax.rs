//! MiniMax: speech on `POST /v1/t2a_v2` and songs on
//! `POST /v1/music_generation`.
//!
//! Both answer HTTP 200 with the outcome in `base_resp.status_code`, and the
//! audio hex-encoded in `data.audio` when it succeeded. The adapter asks for
//! `output_format: hex` so the audio arrives in the answer instead of behind
//! a link that expires, decodes it, and classifies a refusal from the status
//! code MiniMax documents.

use serde_json::{json, Map, Value};

use super::super::super::outcome::refusal::{provider_refused, transport_refusal};
use super::super::super::outcome::response_body::response_text;
use super::super::super::outcome::typed::typed_object;
use super::super::audio::{audio, SpokenAudio};
use super::super::call::Call;
use crate::types::{GatewayRefusal, ProviderRefusal, Refusal};

/// What MiniMax encodes when the request names no `audio_setting.format`:
/// its own documented default, so the answer is labelled as what MiniMax
/// actually sent.
/// <https://platform.minimax.io/docs/api-reference/speech-t2a-http>
const DEFAULT_FORMAT: &str = "mp3";

/// MiniMax's `base_resp.status_code` values, each as MiniMax's error-code page states it.
mod status {
    /// The request succeeded. <https://platform.minimax.io/docs/api-reference/errorcode>
    pub(super) const SUCCESS: i64 = 0;
    /// Unknown error on MiniMax's side. <https://platform.minimax.io/docs/api-reference/errorcode>
    pub(super) const UNKNOWN: i64 = 1000;
    /// MiniMax did not finish the request in its own time. <https://platform.minimax.io/docs/api-reference/errorcode>
    pub(super) const UNFINISHED: i64 = 1001;
    /// Rate limit triggered. <https://platform.minimax.io/docs/api-reference/errorcode>
    pub(super) const RATE_LIMITED: i64 = 1002;
    /// Authentication failed. <https://platform.minimax.io/docs/api-reference/errorcode>
    pub(super) const UNAUTHENTICATED: i64 = 1004;
    /// Insufficient account balance. <https://platform.minimax.io/docs/api-reference/errorcode>
    pub(super) const INSUFFICIENT_BALANCE: i64 = 1008;
    /// Internal service error. <https://platform.minimax.io/docs/api-reference/errorcode>
    pub(super) const INTERNAL: i64 = 1013;
    /// Token limit triggered. <https://platform.minimax.io/docs/api-reference/errorcode>
    pub(super) const TOKEN_LIMITED: i64 = 1039;
}

pub(in super::super) async fn speak(
    call: &Call<'_>,
    payload: Map<String, Value>,
) -> Result<SpokenAudio, Refusal> {
    if payload.contains_key("instructions") {
        return Err(Refusal::gateway(
            GatewayRefusal::InvalidRequest,
            format!(
                "invalid_request: route `{}` takes no `instructions` option",
                call.route_id
            ),
        ));
    }
    let mut voice_setting = Map::new();
    if let Some(voice) = payload.get("voice") {
        voice_setting.insert("voice_id".to_string(), voice.clone());
    }
    if let Some(speed) = payload.get("speed") {
        voice_setting.insert("speed".to_string(), speed.clone());
    }
    if let Some(emotion) = payload.get("emotion") {
        voice_setting.insert("emotion".to_string(), emotion.clone());
    }
    let format = requested_format(&payload)?;
    let body = json!({
        "model": call.model_id,
        "text": payload.get("input"),
        "stream": false,
        "output_format": "hex",
        "voice_setting": voice_setting,
        "audio_setting": {"format": format},
    });
    let response = stado_wait::until(
        stado_wait::Kind::Network,
        format!("minimax speech for `{}`", call.route_id),
        call.descriptor.speech_path,
        call.post(call.descriptor.speech_path).json(&body).send(),
    )
    .await
    .map_err(|error| transport_refusal(&error))?;
    hex_audio(call.route_id, response, &format).await
}

pub(in super::super) async fn compose(
    call: &Call<'_>,
    payload: Map<String, Value>,
) -> Result<SpokenAudio, Refusal> {
    let format = requested_format(&payload)?;
    let mut body = json!({
        "model": call.model_id,
        "lyrics": payload.get("lyrics"),
        "output_format": "hex",
        "audio_setting": {"format": format},
    });
    if let Some(prompt) = payload.get("prompt") {
        body["prompt"] = prompt.clone();
    }
    let response = stado_wait::until(
        stado_wait::Kind::Network,
        format!("minimax music for `{}`", call.route_id),
        call.descriptor.music_path,
        call.post(call.descriptor.music_path).json(&body).send(),
    )
    .await
    .map_err(|error| transport_refusal(&error))?;
    hex_audio(call.route_id, response, &format).await
}

/// The format the caller names, MiniMax's own default when it names none, or
/// a refusal when `response_format` is given as something other than text.
fn requested_format(payload: &Map<String, Value>) -> Result<String, Refusal> {
    match payload.get("response_format") {
        None => Ok(DEFAULT_FORMAT.to_string()),
        Some(Value::String(format)) => Ok(format.clone()),
        Some(other) => Err(Refusal::gateway(
            GatewayRefusal::InvalidRequest,
            format!("invalid_request: response_format must be a format name, got {other}"),
        )),
    }
}

/// The audio out of a MiniMax answer, or the refusal it carries.
async fn hex_audio(
    route_id: &str,
    response: reqwest::Response,
    format: &str,
) -> Result<SpokenAudio, Refusal> {
    let (status, _plan, text) = response_text(response).await?;
    if !status.is_success() {
        return Err(provider_refused(route_id, status, &text));
    }
    let body = typed_object(&text)?;
    let code = body
        .pointer("/base_resp/status_code")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            Refusal::new(
                ProviderRefusal::ProviderFailure,
                "provider_failure: minimax answered without base_resp.status_code",
            )
        })?;
    if code != status::SUCCESS {
        let message = match body.pointer("/base_resp/status_msg").and_then(Value::as_str) {
            Some(message) => message.to_string(),
            None => format!("and no base_resp.status_msg; the answer was {text}"),
        };
        return Err(Refusal::new(
            refusal_kind(code),
            format!("provider_failure: minimax refused `{route_id}` with status {code}: {message}"),
        ));
    }
    let encoded = body
        .pointer("/data/audio")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            Refusal::new(
                ProviderRefusal::ProviderFailure,
                "provider_failure: minimax answered success without data.audio",
            )
        })?;
    let bytes = hex::decode(encoded).map_err(|error| {
        Refusal::new(
            ProviderRefusal::ProviderFailure,
            format!("provider_failure: minimax audio is not hex: {error}"),
        )
    })?;
    audio(format!("audio/{format}"), bytes)
}

/// MiniMax's documented status codes, by what they mean for the caller.
fn refusal_kind(code: i64) -> ProviderRefusal {
    match code {
        status::RATE_LIMITED | status::TOKEN_LIMITED => ProviderRefusal::RateLimited,
        status::UNAUTHENTICATED => ProviderRefusal::Authentication,
        status::INSUFFICIENT_BALANCE => ProviderRefusal::QuotaExhausted,
        status::UNKNOWN | status::UNFINISHED | status::INTERNAL => ProviderRefusal::DependencyUnavailable,
        _ => ProviderRefusal::ProviderFailure,
    }
}
