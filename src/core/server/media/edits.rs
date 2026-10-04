//! `POST /v1/images/edits`: the OpenAI edit contract, a `multipart/form-data`
//! form whose text fields are the generation options and whose files are the
//! input images (`image`, or `image[]` for several) and an optional `mask`.
//!
//! The form is read into the same request `POST /v1/images/generations`
//! takes, the files becoming `data:` URLs in its `image` and `mask` fields,
//! and is served by the same path: the model is the deployment's image alias
//! or a canonical route the bearer allows, and the provider behind it edits
//! on its own edit endpoint or reads the images in its generation request.
//! A client written against the OpenAI SDK's `images.edit` reaches Brama
//! unchanged.

use axum::body::Bytes;
use axum::extract::Extension;
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use serde_json::{Map, Value};

use super::multipart::{boundary, parts, Part};
use super::requests::ImageRequest;
use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::refusal::{api_error, ApiError};

/// The form fields that are numbers or flags in the generation request.
const INTEGER_FIELDS: &[&str] = &["n", "seed"];
const FLAG_FIELDS: &[&str] = &["watermark"];

pub(in crate::core::server) async fn image_edits(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let request = edit_request(&headers, &body)
        .map_err(|reason| api_error(StatusCode::BAD_REQUEST, &reason))?;
    super::image(&client_identity, &aliases, request).await
}

/// The edit form, read into the generation request it asks for. An option
/// the generation request does not take is refused by name, as it is there.
fn edit_request(headers: &HeaderMap, body: &[u8]) -> Result<ImageRequest, String> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let boundary = boundary(content_type).ok_or_else(|| {
        format!(
            "POST /v1/images/edits takes multipart/form-data with a boundary, not \
             `{content_type}`; JSON with input images as data: URLs in `image` goes to \
             POST /v1/images/generations"
        )
    })?;
    let mut fields = Map::new();
    let mut images = Vec::new();
    for part in parts(body, boundary)? {
        match (part.name.as_str(), part.filename.is_some()) {
            ("image" | "image[]", true) => {
                let field = format!("image[{}]", images.len());
                images.push(Value::String(data_url(&part, &field)?));
            }
            ("mask", true) => {
                fields.insert("mask".to_string(), Value::String(data_url(&part, "mask")?));
            }
            (name, true) => {
                return Err(format!(
                    "`{name}` is a file field this endpoint does not take; files are `image`, \
                     `image[]` and `mask`"
                ))
            }
            (name, false) => {
                let text = std::str::from_utf8(part.body)
                    .map_err(|_| format!("`{name}` is not UTF-8 text"))?;
                fields.insert(name.to_string(), field_value(name, text)?);
            }
        }
    }
    if images.is_empty() {
        return Err("an edit carries at least one input image in `image` or `image[]`".to_string());
    }
    fields.insert("image".to_string(), Value::Array(images));
    serde_json::from_value(Value::Object(fields)).map_err(|error| error.to_string())
}

/// One text field as the generation request types it.
fn field_value(name: &str, text: &str) -> Result<Value, String> {
    if INTEGER_FIELDS.contains(&name) {
        return text
            .trim()
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| format!("`{name}` is a whole number, not `{text}`"));
    }
    if FLAG_FIELDS.contains(&name) {
        return text
            .trim()
            .parse::<bool>()
            .map(Value::Bool)
            .map_err(|_| format!("`{name}` is true or false, not `{text}`"));
    }
    Ok(Value::String(text.to_string()))
}

/// One uploaded image as the `data:` URL the generation request carries.
fn data_url(part: &Part<'_>, field: &str) -> Result<String, String> {
    let content_type = part
        .content_type
        .as_deref()
        .ok_or_else(|| format!("{field} names no Content-Type"))?;
    if !content_type.starts_with("image/") {
        return Err(format!("{field} is `{content_type}`, not an image type"));
    }
    if part.body.is_empty() {
        return Err(format!("{field} holds no image"));
    }
    Ok(format!(
        "data:{content_type};base64,{}",
        BASE64.encode(part.body)
    ))
}
