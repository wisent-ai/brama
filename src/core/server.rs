use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tracing::{info, warn};

use crate::subscription_dispatch::{
    dispatch_any_subscription, dispatch_any_vision_capable_subscription, dispatch_subscription,
    dispatch_task_subscription, is_subscription_model, registry_models_for_agent,
};
use crate::types::{BillingTarget, Message, ModelRequest, Tool, ToolCall};

static TOTAL_REQUESTS: AtomicU64 = AtomicU64::new(0);
static TOTAL_INPUT_TOKENS: AtomicU64 = AtomicU64::new(0);
static TOTAL_OUTPUT_TOKENS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize)]
struct ChatCompletionRequest {
    #[serde(default)]
    model: Option<String>,
    messages: Vec<ChatMessage>,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
    #[serde(default = "default_temperature")]
    temperature: f64,
    #[serde(default)]
    tools: Option<Vec<Tool>>,
    #[serde(default, rename = "billingTarget")]
    billing_target: Option<BillingTarget>,
    #[serde(default, rename = "subscriptionDecisionId")]
    subscription_decision_id: Option<String>,
}

fn default_max_tokens() -> u32 {
    1024
}
fn default_temperature() -> f64 {
    0.7
}

fn is_any_subscription_selector(model: &str) -> bool {
    model.trim().eq_ignore_ascii_case("any")
}

fn is_any_vision_capable_subscription_selector(model: &str) -> bool {
    model.trim().eq_ignore_ascii_case("any-vision-capable")
}

fn task_subscription_selector(model: &str) -> Option<String> {
    model
        .trim()
        .strip_prefix("task:")
        .map(str::trim)
        .filter(|task| !task.is_empty())
        .map(String::from)
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    role: String,
    #[serde(default)]
    content: Option<serde_json::Value>,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize)]
struct ChatCompletionResponse {
    id: String,
    object: String,
    model: String,
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Debug, Serialize)]
struct Choice {
    index: u32,
    message: ChoiceMessage,
    finish_reason: String,
}

#[derive(Debug, Serialize)]
struct ChoiceMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Serialize)]
struct Usage {
    prompt_tokens: u32,

    completion_tokens: u32,
    total_tokens: u32,
}

fn model_error_status(message: &str) -> StatusCode {
    let normalized = message.to_ascii_lowercase();
    if normalized.starts_with("auth:")
        || normalized.contains("missing x-agent-")
        || normalized.contains("no auth secret for agent")
    {
        return StatusCode::UNAUTHORIZED;
    }
    if [
        "selected subscription",
        "all '",
        "hit your limit",
        "session limit",
        "usage limit",
        "weekly limit",
        "rate_limit",
        "authentication_error",
        "invalid authentication",
        "refresh token was revoked",
        "access token could not be refreshed",
        "invalid_grant",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
    {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    StatusCode::INTERNAL_SERVER_ERROR
}

async fn chat_completions(
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    let req: ChatCompletionRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": {
                        "message": format!("invalid JSON: {e}"),
                        "type": "bad_request",
                    }
                })),
            );
        }
    };

    let messages: Vec<Message> = req
        .messages
        .into_iter()
        .map(|m| Message {
            role: m.role,
            content: m
                .content
                .unwrap_or_else(|| serde_json::Value::String(String::new())),
            tool_call_id: m.tool_call_id,
            name: m.name,
            tool_calls: m.tool_calls,
        })
        .collect();

    let requested_model = req.model.as_deref().unwrap_or("").trim();
    if requested_model.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": "missing field `model`",
                    "type": "bad_request",
                }
            })),
        );
    }
    // Legacy alias used by external reauth probes: ride the canonical
    // claude-code subscription rotation for the agent's donated credentials.
    let requested_model = if requested_model == "claude-code-subscription" {
        "claude-code/claude-sonnet-4-6"
    } else {
        requested_model
    };
    let any_subscription = is_any_subscription_selector(requested_model);
    let any_vision_capable_subscription =
        is_any_vision_capable_subscription_selector(requested_model);
    let task_subscription = task_subscription_selector(requested_model);
    let selected_model = requested_model.to_string();
    let subscription_request = any_subscription
        || any_vision_capable_subscription
        || task_subscription.is_some()
        || is_subscription_model(&selected_model);
    if !subscription_request {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "message": "model must be a canonical provider/model route or a supported selector",
                    "type": "bad_request",
                }
            })),
        );
    }

    let resp = {
        // Preserve the OpenAI system-role message as ModelRequest.system while
        // the stateless provider adapter receives the remaining conversation.
        let (system, non_system_messages): (Option<String>, Vec<Message>) = {
            let mut sys: Option<String> = None;
            let mut rest: Vec<Message> = Vec::with_capacity(messages.len());
            for m in messages {
                if m.role == "system" && sys.is_none() {
                    sys = m.content.as_str().map(|s| s.to_string());
                    continue;
                }
                rest.push(m);
            }
            (sys, rest)
        };
        let dispatch_req = ModelRequest {
            messages: non_system_messages,
            model: selected_model.clone(),
            max_tokens: req.max_tokens,
            temperature: req.temperature,
            system,
            tools: req.tools,
            billing_target: req.billing_target,
            subscription_decision_id: req.subscription_decision_id,
        };
        if let Some(task) = task_subscription.as_deref() {
            dispatch_task_subscription(&headers, &dispatch_req, &body, task).await
        } else if any_vision_capable_subscription {
            dispatch_any_vision_capable_subscription(&headers, &dispatch_req, &body).await
        } else if any_subscription {
            dispatch_any_subscription(&headers, &dispatch_req, &body).await
        } else {
            dispatch_subscription(&headers, &dispatch_req, &body).await
        }
    };

    TOTAL_REQUESTS.fetch_add(1, Ordering::Relaxed);
    TOTAL_INPUT_TOKENS.fetch_add(resp.input_tokens as u64, Ordering::Relaxed);
    TOTAL_OUTPUT_TOKENS.fetch_add(resp.output_tokens as u64, Ordering::Relaxed);
    if !resp.success {
        let message = resp.error.unwrap_or_default();
        let status = model_error_status(&message);
        let body = json!({
            "error": {
                "message": message,
                "type": if status == StatusCode::TOO_MANY_REQUESTS {
                    "subscription_unavailable"
                } else {
                    "server_error"
                },
            }
        });
        return (status, Json(body));
    }

    // Telemetry is keyed by the requested route id, not the upstream model the
    // subscription rotation actually served, so it joins with catalog ids.
    crate::core::perf::record(&selected_model, resp.latency_ms, resp.output_tokens);

    let has_tool_calls = resp.tool_calls.as_ref().map_or(false, |tc| !tc.is_empty());
    let finish_reason = if has_tool_calls { "tool_calls" } else { "stop" };

    let body = serde_json::to_value(ChatCompletionResponse {
        id: format!("chatcmpl-{}", uuid_v4()),
        object: "chat.completion".into(),
        model: resp.model,
        choices: vec![Choice {
            index: 0,
            message: ChoiceMessage {
                role: "assistant".into(),
                content: resp.content,
                tool_calls: resp.tool_calls,
            },
            finish_reason: finish_reason.into(),
        }],
        usage: Usage {
            prompt_tokens: resp.input_tokens,
            completion_tokens: resp.output_tokens,
            total_tokens: resp.input_tokens + resp.output_tokens,
        },
    })
    .unwrap_or_default();

    (StatusCode::OK, Json(body))
}

async fn health() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

/// Optional per-model telemetry block, present only when the route has stats.
fn perf_json(model: &str) -> Option<serde_json::Value> {
    crate::core::perf::get(model).map(|perf| {
        json!({
            "count": perf.count,
            "latencyMs": perf.latency_ms,
            "tps": perf.tps,
            "lastLatencyMs": perf.last_latency_ms,
            "lastTps": perf.last_tps,
        })
    })
}

async fn list_models(headers: axum::http::HeaderMap) -> impl IntoResponse {
    let mut model_ids = Vec::new();
    let mut available = HashSet::new();
    let catalog_agent =
        std::env::var("BRAMA_CATALOG_AGENT_ID").unwrap_or_else(|_| "wisent-app".into());

    let mut registry_metadata = HashMap::new();
    let mut catalog_revision =
        std::env::var("BRAMA_CATALOG_REVISION").unwrap_or_else(|_| "brama-v1".into());
    let mut degraded = false;
    match crate::subscription_dispatch::model_catalog::snapshot().await {
        Ok(catalog) => {
            catalog_revision = catalog.revision.clone();
            for model in &catalog.models {
                model_ids.push(model.route_id.clone());
                registry_metadata.insert(model.route_id.clone(), model.clone());
            }
        }
        Err(error) => {
            degraded = true;
            warn!(%error, "public model catalog unavailable");
        }
    }
    match registry_models_for_agent(&catalog_agent).await {
        Ok(models) => {
            for model in models {
                available.insert(model.route_id.clone());
                model_ids.push(model.route_id.clone());
                registry_metadata.insert(model.route_id.clone(), model);
            }
        }
        Err(error) => {
            degraded = true;
            warn!(%error, "native provider model discovery failed");
        }
    }
    model_ids.sort();
    model_ids.dedup();

    if headers.contains_key("x-jeden-schema-min") {
        let models = model_ids
            .into_iter()
            .map(|id| {
                let registry = registry_metadata.get(&id);
                let input_modalities = registry
                    .map(|model| model.input_modalities.clone())
                    .filter(|modalities| !modalities.is_empty())
                    .unwrap_or_else(|| vec!["text".to_string()]);
                let context_window = registry.map_or(200_000, |model| model.context_window);
                let max_output_tokens = registry.map_or(32_000, |model| model.max_output_tokens);
                let tools = registry.is_some_and(|model| model.tools);
                let reasoning = registry.is_some_and(|model| model.reasoning);
                let price = registry.map_or((0.0, 0.0, 0.0, 0.0), |model| {
                    (
                        model.input_price,
                        model.output_price,
                        model.cache_read_price,
                        model.cache_write_price,
                    )
                });
                let mut entry = json!({
                    "id": id,
                    "available": available.contains(&id),
                    "contextWindow": context_window,
                    "maxOutputTokens": max_output_tokens,
                    "inputModalities": input_modalities,
                    "outputModalities": ["text"],
                    "tools": tools,
                    "reasoning": reasoning,
                    "price": {
                        "input": price.0,
                        "output": price.1,
                        "cacheRead": price.2,
                        "cacheWrite": price.3,
                    },
                    "fallback": [],
                    "promotion": [],
                });
                if let Some(perf) = perf_json(&id) {
                    entry["perf"] = perf;
                }
                entry
            })
            .collect::<Vec<_>>();
        return Json(json!({
            "catalogRevision": catalog_revision,
            "version": "v1",
            "models": models,
            "degraded": degraded,
        }));
    }
    let models = model_ids
        .into_iter()
        .map(|id| {
            let owner = registry_metadata
                .get(&id)
                .map(|model| model.provider_id.as_str())
                .unwrap_or("brama");
            let mut entry = json!({
                "id": id,
                "object": "model",
                "owned_by": owner,
            });
            if let Some(perf) = perf_json(&id) {
                entry["perf"] = perf;
            }
            entry
        })
        .collect::<Vec<_>>();
    Json(json!({
        "object": "list",
        "data": models,
    }))
}

async fn get_stats() -> impl IntoResponse {
    Json(json!({
        "total_requests": TOTAL_REQUESTS.load(Ordering::Relaxed),
        "total_input_tokens": TOTAL_INPUT_TOKENS.load(Ordering::Relaxed),
        "total_output_tokens": TOTAL_OUTPUT_TOKENS.load(Ordering::Relaxed),
        "perfModels": crate::core::perf::tracked_count(),
    }))
}

#[derive(Debug, Deserialize)]
struct DonateSubscriptionRequest {
    user_id: Option<String>,
    provider: Option<String>,
    label: Option<String>,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RetireSubscriptionRequest {
    user_id: Option<String>,
    subscription_id: Option<String>,
}

type ApiError = (StatusCode, Json<serde_json::Value>);

fn api_error(status: StatusCode, message: &str) -> ApiError {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": "bad_request",
            }
        })),
    )
}

/// Mutating subscription endpoints accept any donor identity unless
/// BRAMA_DONOR_USER_ID opts into an exact-match check (legacy-service parity).
fn donor_authorized(user_id: Option<&str>) -> Result<(), ApiError> {
    let expected = std::env::var("BRAMA_DONOR_USER_ID")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let Some(expected) = expected else {
        eprintln!(
            "[subscriptions] donation accepted without donor check (BRAMA_DONOR_USER_ID unset), user_id={}",
            user_id.unwrap_or("")
        );
        return Ok(());
    };
    if user_id != Some(expected.as_str()) {
        return Err(api_error(StatusCode::FORBIDDEN, "forbidden"));
    }
    Ok(())
}

async fn list_agent_subscriptions(Path(agent_id): Path<String>) -> impl IntoResponse {
    let subscriptions = crate::gateway::broker::list_subscriptions(&agent_id)
        .await
        .into_iter()
        .map(|entry| {
            json!({
                "id": entry.id,
                "provider": entry.provider,
                "status": entry.status,
            })
        })
        .collect::<Vec<_>>();
    Json(json!({"subscriptions": subscriptions}))
}

async fn donate_subscription(
    Path(agent_id): Path<String>,
    Json(req): Json<DonateSubscriptionRequest>,
) -> ApiError {
    if let Err(error) = donor_authorized(req.user_id.as_deref()) {
        return error;
    }
    let provider = match req.provider.as_deref().map(str::trim) {
        Some("claude_code" | "claude-code") => "claude-code",
        _ => return api_error(StatusCode::BAD_REQUEST, "provider must be claude_code"),
    };
    let api_key = req.api_key.as_deref().unwrap_or("");
    if api_key.is_empty() || api_key.chars().count() > 8000 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "api_key must contain 1..8000 characters",
        );
    }
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let subscription_id = format!(
        "brama-sub-{}-claude-{millis}",
        crate::gateway::broker::slug(&agent_id)
    );
    if let Err(message) =
        crate::gateway::broker::put_donated_credential(&subscription_id, api_key).await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": {
                    "message": message,
                    "type": "server_error",
                }
            })),
        );
    }
    if let Err(message) =
        crate::gateway::broker::donated_add(&agent_id, &subscription_id, provider)
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": {
                    "message": message,
                    "type": "server_error",
                }
            })),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "subscription": {
                "id": subscription_id,
                "provider": provider,
                "agent_id": agent_id,
                "status": "active",
                "label": req.label,
            }
        })),
    )
}

async fn retire_subscription(
    Path(_agent_id): Path<String>,
    Json(req): Json<RetireSubscriptionRequest>,
) -> ApiError {
    if let Err(error) = donor_authorized(req.user_id.as_deref()) {
        return error;
    }
    let subscription_id = req.subscription_id.as_deref().map(str::trim).unwrap_or("");
    if subscription_id.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "subscription_id is required");
    }
    crate::journal::retire(subscription_id);
    if let Err(message) = crate::gateway::broker::donated_remove(subscription_id) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": {
                    "message": message,
                    "type": "server_error",
                }
            })),
        );
    }
    (StatusCode::OK, Json(json!({"ok": true})))
}

pub async fn start_server(port: u16) -> Result<(), std::io::Error> {
    // Touch the perf registry so persisted stats load at startup, not on first use.
    info!(models = crate::core::perf::tracked_count(), "perf registry loaded");

    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .route(
            "/v1/subscriptions/:agent_id",
            get(list_agent_subscriptions)
                .post(donate_subscription)
                .delete(retire_subscription),
        )
        .route("/health", get(health))
        .route("/stats", get(get_stats));

    let addr = format!("0.0.0.0:{port}");
    info!("Starting brama server on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{t:032x}")
}
