use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;
use tracing::{info, warn};

use super::router::ModelRouter;
use crate::gateway::broker;
use crate::subscription_dispatch::{
    authenticate_agent, codex_models_for_agent, dispatch_any_subscription,
    dispatch_any_vision_capable_subscription, dispatch_subscription, dispatch_task_subscription,
    is_subscription_model, subscription_model_for_provider,
};
use crate::types::{BillingTarget, Message, ModelRequest, Tool, ToolCall};

type SharedRouter = Arc<RwLock<ModelRouter>>;

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
    State(router): State<SharedRouter>,
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
        if let Err(error) = authenticate_agent(&headers, &body).await {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "error": {
                        "message": error,
                        "type": "authentication_error",
                    }
                })),
            );
        }
    }

    let resp = if subscription_request {
        // OpenAI-compat callers send the system prompt as a system-role
        // message in `messages`. The subscription engines (claude_code,
        // codex, kimi, opencode) read `request.system` as a separate
        // field and `build_prompt_from` drops system-role messages on the
        // floor. Pull the first system-role message's text out and set
        // it on `system` so it actually reaches the CLI.
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
    } else {
        let r = router.read().await;
        r.complete(
            messages,
            &selected_model,
            req.max_tokens,
            req.temperature,
            None,
            req.tools,
        )
        .await
    };

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

async fn list_models(
    State(router): State<SharedRouter>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let mut model_ids = {
        let router = router.read().await;
        router.all_models()
    };
    let catalog_agent =
        std::env::var("BRAMA_CATALOG_AGENT_ID").unwrap_or_else(|_| "wisent-app".into());
    let mut has_codex_subscription = false;
    for subscription in broker::list_subscriptions(&catalog_agent).await {
        if subscription.status == "active" {
            if let Some(model) = subscription_model_for_provider(&subscription.provider) {
                has_codex_subscription |= model == "codex-subscription";
                model_ids.push(model.to_owned());
            }
        }
    }

    let mut codex_metadata = HashMap::new();
    let mut degraded = false;
    if has_codex_subscription {
        match codex_models_for_agent(&catalog_agent).await {
            Ok(models) => {
                for model in models {
                    model_ids.push(model.id.clone());
                    codex_metadata.insert(model.id.clone(), model);
                }
            }
            Err(error) => {
                degraded = true;
                warn!(%error, "Codex model discovery failed");
            }
        }
    }
    model_ids.sort();
    model_ids.dedup();

    if headers.contains_key("x-jeden-schema-min") {
        let models = model_ids
            .into_iter()
            .map(|id| {
                let codex = codex_metadata.get(&id);
                let input_modalities = codex
                    .map(|model| model.input_modalities.clone())
                    .filter(|modalities| !modalities.is_empty())
                    .unwrap_or_else(|| vec!["text".to_string()]);
                let reasoning = codex.is_some_and(|model| model.reasoning);
                json!({
                    "id": id,
                    "available": true,
                    "contextWindow": 200_000,
                    "maxOutputTokens": 32_000,
                    "inputModalities": input_modalities,
                    "outputModalities": ["text"],
                    "tools": false,
                    "reasoning": reasoning,
                    "price": {
                        "input": 0.0,
                        "output": 0.0,
                        "cacheRead": 0.0,
                        "cacheWrite": 0.0,
                    },
                    "fallback": [],
                    "promotion": [],
                })
            })
            .collect::<Vec<_>>();
        return Json(json!({
            "catalogRevision": std::env::var("BRAMA_CATALOG_REVISION")
                .unwrap_or_else(|_| "brama-v1".into()),
            "version": "v1",
            "models": models,
            "degraded": degraded,
        }));
    }
    let models = model_ids
        .into_iter()
        .map(|id| {
            json!({
                "id": id,
                "object": "model",
                "owned_by": "brama",
            })
        })
        .collect::<Vec<_>>();
    Json(json!({
        "object": "list",
        "data": models,
    }))
}

async fn get_stats(State(router): State<SharedRouter>) -> impl IntoResponse {
    let r = router.read().await;
    let reqs = r.stats.total_requests.load(Ordering::Relaxed);
    let inp = r.stats.total_input_tokens.load(Ordering::Relaxed);
    let out = r.stats.total_output_tokens.load(Ordering::Relaxed);

    Json(json!({
        "total_requests": reqs,
        "total_input_tokens": inp,
        "total_output_tokens": out,
    }))
}

pub async fn start_server(router: ModelRouter, port: u16) -> Result<(), std::io::Error> {
    let shared: SharedRouter = Arc::new(RwLock::new(router));

    let chat_app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .route("/health", get(health))
        .route("/stats", get(get_stats))
        .with_state(shared);

    let app = chat_app;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_camel_case_subscription_routing_fields() {
        let request: ChatCompletionRequest = serde_json::from_value(json!({
            "model": "claude-code-subscription",
            "messages": [{"role": "user", "content": "hello"}],
            "billingTarget": {
                "providerId": "claude_code",
                "accountId": "acct-1",
                "subscriptionId": "sub-2",
                "quotaBucket": "chat"
            },
            "subscriptionDecisionId": "decision-3"
        }))
        .expect("request must deserialize");

        assert_eq!(
            request.billing_target,
            Some(BillingTarget {
                provider_id: "claude_code".into(),
                account_id: "acct-1".into(),
                subscription_id: "sub-2".into(),
            })
        );
        assert_eq!(
            request.subscription_decision_id.as_deref(),
            Some("decision-3")
        );
    }

    #[test]
    fn maps_subscription_burnout_to_rotation_signal() {
        assert_eq!(
            model_error_status("selected subscription 'sub-2' is not active"),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            model_error_status("auth: invalid request signature"),
            StatusCode::UNAUTHORIZED
        );
    }
}
