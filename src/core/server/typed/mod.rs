//! The two endpoints that accept exactly one model name each:
//! `POST /v1/embeddings` and `POST /v1/moderations`.
//!
//! Neither routes: the alias names the destination, the call goes straight out
//! in the OpenAI typed shape, and the answer is handed back whole. That is why
//! they share nothing with the chat path except the counters and the refusal
//! document.

mod requests;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Map, Value};

use crate::core::server::admission::identity::ModelClientIdentity;
use crate::core::server::aliases::table::ModelAliases;
use crate::core::server::aliases::{WISENT_EMBEDDING_ALIAS, WISENT_MODERATION_ALIAS};
use crate::core::server::refusal::contract::model_error_contract;
use crate::core::server::refusal::envelope::{typed_dispatch_attempts, typed_dispatch_error};
use crate::core::server::refusal::{api_error, ApiError};
use crate::core::server::telemetry::{record_typed_request, record_typed_usage};
use crate::subscription_dispatch::dispatch_direct_openai_typed;

use requests::{EmbeddingRequest, ModerationRequest};

pub(in crate::core::server) async fn embeddings(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<EmbeddingRequest>,
) -> Result<Json<Value>, ApiError> {
    if request.model != WISENT_EMBEDDING_ALIAS
        || !client_identity.authorizes_model(&request.model)
        || !request.input.is_valid()
        || request
            .dimensions
            .is_some_and(|value| value == u32::default())
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid embedding request",
        ));
    }
    let source = aliases
        .source(WISENT_EMBEDDING_ALIAS)
        .ok_or_else(|| api_error(StatusCode::INTERNAL_SERVER_ERROR, "embedding alias missing"))?;
    let mut payload = Map::new();
    payload.insert(
        "input".to_string(),
        serde_json::to_value(request.input)
            .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid embedding input"))?,
    );
    if let Some(value) = request.encoding_format {
        if !matches!(value.as_str(), "float" | "base64") {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "invalid embedding encoding format",
            ));
        }
        payload.insert("encoding_format".to_string(), Value::String(value));
    }
    if let Some(value) = request.dimensions {
        payload.insert("dimensions".to_string(), json!(value));
    }
    if let Some(value) = request.user {
        if value.is_empty() {
            return Err(api_error(StatusCode::BAD_REQUEST, "invalid embedding user"));
        }
        payload.insert("user".to_string(), Value::String(value));
    }
    let body = match dispatch_direct_openai_typed(&source, "/v1/embeddings", payload).await {
        Ok(body) => body,
        Err(message) => {
            let attempts = typed_dispatch_attempts(model_error_contract(&message));
            record_typed_request(attempts, true);
            return Err(typed_dispatch_error(&message));
        }
    };
    record_typed_usage(&body);
    if !body.get("data").is_some_and(Value::is_array) {
        record_typed_request(u32::from(true), true);
        return Err(typed_dispatch_error(
            "embedding provider returned malformed data",
        ));
    }
    record_typed_request(u32::from(true), false);
    Ok(Json(body))
}

pub(in crate::core::server) async fn moderations(
    Extension(client_identity): Extension<ModelClientIdentity>,
    Extension(aliases): Extension<ModelAliases>,
    Json(request): Json<ModerationRequest>,
) -> Result<Json<Value>, ApiError> {
    if request.model != WISENT_MODERATION_ALIAS
        || !client_identity.authorizes_model(&request.model)
        || !request.input.is_valid()
    {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid moderation request",
        ));
    }
    let source = aliases.source(WISENT_MODERATION_ALIAS).ok_or_else(|| {
        api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "moderation alias missing",
        )
    })?;
    let mut payload = Map::new();
    payload.insert(
        "input".to_string(),
        serde_json::to_value(request.input)
            .map_err(|_| api_error(StatusCode::BAD_REQUEST, "invalid moderation input"))?,
    );
    let body = match dispatch_direct_openai_typed(&source, "/v1/moderations", payload).await {
        Ok(body) => body,
        Err(message) => {
            let attempts = typed_dispatch_attempts(model_error_contract(&message));
            record_typed_request(attempts, true);
            return Err(typed_dispatch_error(&message));
        }
    };
    record_typed_usage(&body);
    if !body.get("results").is_some_and(Value::is_array) {
        record_typed_request(u32::from(true), true);
        return Err(typed_dispatch_error(
            "moderation provider returned malformed data",
        ));
    }
    record_typed_request(u32::from(true), false);
    Ok(Json(body))
}
