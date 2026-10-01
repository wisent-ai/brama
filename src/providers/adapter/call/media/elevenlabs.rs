//! ElevenLabs: speech, the voice library and voice cloning.
//!
//! Speech is `POST /v1/text-to-speech/{voice}` with the text, the model and
//! the voice settings in the body and the encoding in `output_format`; the
//! answer is the audio. With `timestamps` the call goes to
//! `…/with-timestamps`, whose answer is JSON — the audio as `audio_base64`
//! beside the character `alignment` — and that JSON is handed back as it
//! arrived. The library is `GET /v1/voices`, and a clone is
//! `POST /v1/voices/add` with the recordings as multipart files.

use serde_json::{json, Map, Value};

use super::super::outcome::refusal::{provider_refused, transport_refusal};
use super::super::outcome::response_body::bounded_response_text;
use super::super::outcome::typed::typed_object;
use super::audio::{audio, audio_answer, Call, SpokenAudio, VoiceSample};
use super::{answered, valid_path_segment};
use crate::types::{GatewayRefusal, ProviderRefusal, Refusal};

/// The OpenAI-shaped option ElevenLabs has no counterpart for.
const UNSUPPORTED: &[&str] = &["instructions"];
/// The request fields that travel inside `voice_settings`.
const VOICE_SETTINGS: &[&str] = &["stability", "similarity_boost", "speed"];

pub(super) async fn speak(
    call: &Call<'_>,
    payload: Map<String, Value>,
) -> Result<SpokenAudio, Refusal> {
    if let Some(option) = UNSUPPORTED
        .iter()
        .find(|option| payload.contains_key(**option))
    {
        return Err(invalid(format!(
            "route `{}` takes no `{option}` option",
            call.route_id
        )));
    }
    let voice = payload
        .get("voice")
        .and_then(Value::as_str)
        .filter(|voice| valid_path_segment(voice))
        .ok_or_else(|| invalid("voice must be an ElevenLabs voice id".to_string()))?;
    let mut body = json!({
        "text": payload.get("input"),
        "model_id": call.model_id,
    });
    let settings = VOICE_SETTINGS
        .iter()
        .filter_map(|key| {
            payload
                .get(*key)
                .map(|value| ((*key).to_string(), value.clone()))
        })
        .collect::<Map<String, Value>>();
    if !settings.is_empty() {
        body["voice_settings"] = Value::Object(settings);
    }
    let timed = payload.get("timestamps") == Some(&Value::Bool(true));
    let mut path = call.descriptor.speech_path.replace("{voice}", voice);
    if timed {
        path.push_str("/with-timestamps");
    }
    let mut request = call.post(&path);
    if let Some(format) = payload.get("response_format").and_then(Value::as_str) {
        request = request.query(&[("output_format", format)]);
    }
    let response = request
        .json(&body)
        .send()
        .await
        .map_err(|error| transport_refusal(&error))?;
    if !timed {
        return audio_answer(call.route_id, response).await;
    }
    let (status, _plan, text) = bounded_response_text(response).await?;
    if !status.is_success() {
        return Err(provider_refused(call.route_id, status, &text));
    }
    let answer = typed_object(&text)?;
    if !answer.get("audio_base64").is_some_and(Value::is_string) {
        return Err(Refusal::new(
            ProviderRefusal::ProviderFailure,
            "provider_failure: elevenlabs answered timed speech without audio_base64",
        ));
    }
    audio("application/json".to_string(), text.into_bytes())
}

/// The account's voices, as ElevenLabs lists them under `voices`.
pub(super) async fn voices(call: &Call<'_>) -> Result<Value, Refusal> {
    let response = call
        .get(call.descriptor.voices_path)
        .send()
        .await
        .map_err(|error| transport_refusal(&error))?;
    answered(call.route_id, response).await
}

/// One voice cloned from recordings; the answer carries its `voice_id`.
///
/// The form is written here rather than by the HTTP client, so the client
/// keeps the small feature set every other provider call uses.
pub(super) async fn clone_voice(
    call: &Call<'_>,
    name: &str,
    description: Option<&str>,
    samples: Vec<VoiceSample>,
) -> Result<Value, Refusal> {
    let boundary = format!("brama-{}", uuid::Uuid::new_v4().simple());
    let mut body = Vec::new();
    form_field(&mut body, &boundary, "name", None, None, name.as_bytes())?;
    if let Some(description) = description {
        form_field(
            &mut body,
            &boundary,
            "description",
            None,
            None,
            description.as_bytes(),
        )?;
    }
    for sample in &samples {
        form_field(
            &mut body,
            &boundary,
            "files",
            Some(&sample.filename),
            Some(&sample.content_type),
            &sample.bytes,
        )?;
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let response = call
        .post(&format!("{}/add", call.descriptor.voices_path))
        .header(
            reqwest::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .await
        .map_err(|error| transport_refusal(&error))?;
    answered(call.route_id, response).await
}

/// One part of a `multipart/form-data` body. A header value that could end
/// its own line or quote is refused, because it would rewrite the form.
fn form_field(
    body: &mut Vec<u8>,
    boundary: &str,
    name: &str,
    filename: Option<&str>,
    content_type: Option<&str>,
    value: &[u8],
) -> Result<(), Refusal> {
    for header in filename.into_iter().chain(content_type) {
        if header.is_empty() || header.contains(['\r', '\n', '"']) {
            return Err(invalid(format!(
                "`{}` cannot be written into a form header",
                header.escape_debug()
            )));
        }
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    let disposition = match filename {
        Some(filename) => {
            format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n")
        }
        None => format!("Content-Disposition: form-data; name=\"{name}\"\r\n"),
    };
    body.extend_from_slice(disposition.as_bytes());
    if let Some(content_type) = content_type {
        body.extend_from_slice(format!("Content-Type: {content_type}\r\n").as_bytes());
    }
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(value);
    body.extend_from_slice(b"\r\n");
    Ok(())
}

fn invalid(reason: String) -> Refusal {
    Refusal::gateway(
        GatewayRefusal::InvalidRequest,
        format!("invalid_request: {reason}"),
    )
}
