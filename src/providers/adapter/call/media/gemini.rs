//! Gemini images: `POST /v1beta/models/{model}:generateContent`.
//!
//! The prompt and every input image travel as parts of one user turn — the
//! images as inline base64 data — and the picture comes back as an inline
//! data part. The answer is rewritten into the OpenAI image shape the image
//! endpoint promises (`data[].b64_json`), with the MIME type Gemini stated
//! beside it.

use serde_json::{json, Map, Value};

use super::super::outcome::refusal::{provider_refused, transport_refusal};
use super::super::outcome::response_body::bounded_response_text;
use super::super::outcome::typed::typed_object;
use super::audio::Call;
use super::valid_path_segment;
use crate::types::{GatewayRefusal, ProviderRefusal, Refusal};

pub(super) async fn generate_image(
    call: &Call<'_>,
    payload: Map<String, Value>,
) -> Result<Value, Refusal> {
    if let Some(option) = payload
        .keys()
        .find(|key| !matches!(key.as_str(), "prompt" | "image" | "aspect_ratio"))
    {
        return Err(invalid(format!(
            "route `{}` takes prompt, image and aspect_ratio, not `{option}`",
            call.route_id
        )));
    }
    if !valid_path_segment(call.model_id) {
        return Err(invalid(format!(
            "`{}` is not a Gemini model id",
            call.model_id
        )));
    }
    let mut parts = vec![json!({"text": payload.get("prompt")})];
    for image in payload
        .get("image")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        parts.push(inline_part(image)?);
    }
    let mut generation = json!({"responseModalities": ["IMAGE"]});
    if let Some(aspect_ratio) = payload.get("aspect_ratio") {
        generation["imageConfig"] = json!({"aspectRatio": aspect_ratio});
    }
    let body = json!({
        "contents": [{"role": "user", "parts": parts}],
        "generationConfig": generation,
    });
    let response = call
        .post(&call.descriptor.image_path.replace("{model}", call.model_id))
        .json(&body)
        .send()
        .await
        .map_err(|error| transport_refusal(&error))?;
    let (status, _plan, text) = bounded_response_text(response).await?;
    if !status.is_success() {
        return Err(provider_refused(call.route_id, status, &text));
    }
    let answer = typed_object(&text)?;
    let picture = answer
        .pointer("/candidates/0/content/parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|part| part.get("inlineData"));
    let Some(picture) = picture else {
        let reason = answer
            .pointer("/promptFeedback/blockReason")
            .or_else(|| answer.pointer("/candidates/0/finishReason"))
            .and_then(Value::as_str)
            .unwrap_or("no reason stated");
        return Err(Refusal::new(
            ProviderRefusal::ProviderFailure,
            format!("provider_failure: gemini returned no image ({reason})"),
        ));
    };
    Ok(json!({
        "created": chrono::Utc::now().timestamp(),
        "model": call.route_id,
        "data": [{
            "b64_json": picture.get("data"),
            "mime_type": picture.get("mimeType"),
        }],
    }))
}

/// One input image as a Gemini inline part. Gemini reads input images only
/// as data it is handed, so a `data:` URL is required.
fn inline_part(image: &Value) -> Result<Value, Refusal> {
    let url = image
        .as_str()
        .ok_or_else(|| invalid("each input image is a data: URL string".to_string()))?;
    let (header, data) = url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(','))
        .ok_or_else(|| invalid("gemini takes input images as data: URLs".to_string()))?;
    let mime_type = header
        .strip_suffix(";base64")
        .ok_or_else(|| invalid("an input image data: URL must be base64-encoded".to_string()))?;
    if !mime_type.starts_with("image/") {
        return Err(invalid(format!("`{mime_type}` is not an image type")));
    }
    Ok(json!({"inline_data": {"mime_type": mime_type, "data": data}}))
}

fn invalid(reason: String) -> Refusal {
    Refusal::gateway(
        GatewayRefusal::InvalidRequest,
        format!("invalid_request: {reason}"),
    )
}
