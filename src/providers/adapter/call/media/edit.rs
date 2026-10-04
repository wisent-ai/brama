//! Picture edits on the OpenAI contract: `POST /v1/images/edits` with the
//! prompt and options as form fields and the input images (and an optional
//! mask) as files.
//!
//! The caller hands Brama its input images as `data:` URLs, in the same
//! `image` field the generation request carries them in, so one request shape
//! reaches every image provider: Gemini and Seedream read them in their
//! generation request, and a provider that declares an edit path is sent
//! this form instead. Brama does not fetch an `https` image on a caller's
//! behalf: an edit route is handed the bytes, never an address to visit.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{Map, Value};

use super::super::outcome::refusal::transport_refusal;
use super::answered;
use super::audio::Call;
use super::form::Form;
use crate::types::{GatewayRefusal, Refusal};

/// Whether this payload asks for an edit rather than a picture from text.
pub(super) fn asks_for_edit(payload: &Map<String, Value>) -> bool {
    payload.contains_key("image") || payload.contains_key("mask")
}

pub(super) async fn edit_image(
    call: &Call<'_>,
    mut payload: Map<String, Value>,
) -> Result<Value, Refusal> {
    let images = match payload.remove("image") {
        Some(Value::Array(images)) if !images.is_empty() => images,
        Some(_) | None => {
            return Err(invalid(format!(
                "route `{}` edits the input images it is handed in `image`, and none was",
                call.route_id
            )))
        }
    };
    let mask = payload.remove("mask");
    let mut form = Form::new();
    form.text("model", call.model_id)?;
    for (name, value) in &payload {
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            Value::Bool(flag) => flag.to_string(),
            _ => {
                return Err(invalid(format!(
                    "`{name}` is not a value an edit form field can carry"
                )))
            }
        };
        form.text(name, &text)?;
    }
    // One image is the `image` field every edit model reads; several are
    // `image[]`, which only the models that take several accept.
    let field = if images.len() == 1 { "image" } else { "image[]" };
    for (index, image) in images.iter().enumerate() {
        let (content_type, bytes) = data_url(image, &format!("image[{index}]"))?;
        form.file(
            field,
            &format!("image-{index}.{}", extension(content_type)),
            content_type,
            &bytes,
        )?;
    }
    if let Some(mask) = &mask {
        let (content_type, bytes) = data_url(mask, "mask")?;
        form.file(
            "mask",
            &format!("mask.{}", extension(content_type)),
            content_type,
            &bytes,
        )?;
    }
    let (content_type, body) = form.finish();
    let response = call
        .post(call.descriptor.image_edit_path)
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .body(body)
        .send()
        .await
        .map_err(|error| transport_refusal(&error))?;
    answered(call.route_id, response).await
}

/// One `data:<image type>;base64,<bytes>` URL, read into its type and bytes.
fn data_url<'a>(value: &'a Value, field: &str) -> Result<(&'a str, Vec<u8>), Refusal> {
    let url = value
        .as_str()
        .ok_or_else(|| invalid(format!("{field} is a data: URL string")))?;
    let (header, data) = url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(','))
        .ok_or_else(|| {
            invalid(format!(
                "{field} must be a data: URL; an edit is handed the image, Brama does not fetch it"
            ))
        })?;
    let content_type = header
        .strip_suffix(";base64")
        .ok_or_else(|| invalid(format!("{field} data: URL must be base64-encoded")))?;
    if !content_type.starts_with("image/") {
        return Err(invalid(format!(
            "{field} is `{content_type}`, not an image type"
        )));
    }
    let bytes = BASE64
        .decode(data.as_bytes())
        .map_err(|_| invalid(format!("{field} data: URL is not valid base64")))?;
    if bytes.is_empty() {
        return Err(invalid(format!("{field} holds no image")));
    }
    Ok((content_type, bytes))
}

/// The file extension a vendor reads the type from, where the form's
/// filename is all it looks at.
fn extension(content_type: &str) -> &str {
    match content_type {
        "image/jpeg" => "jpg",
        other => other.strip_prefix("image/").unwrap_or("png"),
    }
}

fn invalid(reason: String) -> Refusal {
    Refusal::gateway(
        GatewayRefusal::InvalidRequest,
        format!("invalid_request: {reason}"),
    )
}
