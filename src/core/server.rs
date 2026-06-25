use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::info;

use super::router::ModelRouter;
use crate::subscription_dispatch::{
    dispatch_subscription, is_subscription_model,
    SUBSCRIPTION_MODELS,
};
use crate::types::{Message, ModelRequest, Tool, ToolCall};

type SharedRouter = Arc<RwLock<ModelRouter>>;

#[derive(Debug, Deserialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
    #[serde(default = "default_temperature")]
    temperature: f64,
    #[serde(default)]
    tools: Option<Vec<Tool>>,
}

fn default_max_tokens() -> u32 {
    1024
}
fn default_temperature() -> f64 {
    0.7
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

    let resp = if is_subscription_model(&req.model) {
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
            model: req.model.clone(),
            max_tokens: req.max_tokens,
            temperature: req.temperature,
            system,
            tools: req.tools,
        };
        dispatch_subscription(&headers, &dispatch_req, &body).await
    } else {
        let r = router.read().await;
        r.complete(
            messages,
            &req.model,
            req.max_tokens,
            req.temperature,
            None,
            req.tools,
        )
        .await
    };

    if !resp.success {
        let body = json!({
            "error": {
                "message": resp.error.unwrap_or_default(),
                "type": "server_error",
            }
        });
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(body));
    }

    let has_tool_calls = resp
        .tool_calls
        .as_ref()
        .map_or(false, |tc| !tc.is_empty());
    let finish_reason = if has_tool_calls {
        "tool_calls"
    } else {
        "stop"
    };

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
            total_tokens: resp.input_tokens
                + resp.output_tokens,
        },
    })
    .unwrap_or_default();

    (StatusCode::OK, Json(body))
}

async fn health() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

fn model_object(id: &str) -> Value {
    json!({
        "id": id,
        "object": "model",
        "owned_by": "model-router",
    })
}

fn available_model_ids(router: &ModelRouter) -> Vec<String> {
    let mut models = router.all_models();
    models.extend(
        SUBSCRIPTION_MODELS
            .iter()
            .map(|(model, _)| (*model).to_string()),
    );
    models.sort();
    models.dedup();
    models
}

async fn list_models(
    State(router): State<SharedRouter>,
) -> impl IntoResponse {
    let r = router.read().await;
    let models: Vec<Value> = available_model_ids(&r)
        .into_iter()
        .map(|id| model_object(&id))
        .collect();

    Json(json!({
        "object": "list",
        "data": models,
    }))
}

async fn get_model(
    Path(model_id): Path<String>,
    State(router): State<SharedRouter>,
) -> impl IntoResponse {
    let r = router.read().await;
    let exists = available_model_ids(&r)
        .iter()
        .any(|id| id == &model_id);

    if exists {
        return (StatusCode::OK, Json(model_object(&model_id)));
    }

    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": {
                "message": format!("model '{model_id}' not found"),
                "type": "not_found",
            }
        })),
    )
}

async fn get_stats(
    State(router): State<SharedRouter>,
) -> impl IntoResponse {
    let r = router.read().await;
    let reqs =
        r.stats.total_requests.load(Ordering::Relaxed);
    let inp =
        r.stats.total_input_tokens.load(Ordering::Relaxed);
    let out = r
        .stats
        .total_output_tokens
        .load(Ordering::Relaxed);

    Json(json!({
        "total_requests": reqs,
        "total_input_tokens": inp,
        "total_output_tokens": out,
    }))
}

pub async fn start_server(
    router: ModelRouter,
    port: u16,
) -> Result<(), std::io::Error> {
    let shared: SharedRouter =
        Arc::new(RwLock::new(router));

    let chat_app = Router::new()
        .route(
            "/v1/chat/completions",
            post(chat_completions),
        )
        .route("/v1/models", get(list_models))
        .route("/v1/models/:model_id", get(get_model))
        .route("/health", get(health))
        .route("/stats", get(get_stats))
        .with_state(shared);

    let gateway_app = Router::new()
        .route(
            "/v1/subscription-router/:instance_id",
            get(crate::gateway::subscription_router_get),
        )
        .route(
            "/v1/subscriptions/:instance_id",
            get(crate::gateway::subscriptions_get)
                .post(crate::gateway::subscriptions_post)
                .delete(crate::gateway::subscriptions_delete),
        )
        .route(
            "/v1/donate",
            post(crate::gateway::donate_wisent),
        );

    let app = chat_app.merge(gateway_app);

    let addr = format!("0.0.0.0:{port}");
    info!("Starting model-router server on {addr}");

    let listener =
        tokio::net::TcpListener::bind(&addr).await?;
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
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::*;
    use crate::provider::ModelProvider;
    use crate::types::{ModelRequest, ModelResponse};

    struct TestProvider;

    #[async_trait]
    impl ModelProvider for TestProvider {
        async fn complete(
            &self,
            request: &ModelRequest,
        ) -> ModelResponse {
            ModelResponse::failure(
                &request.model,
                "test provider".into(),
            )
        }

        fn estimate_cost(
            &self,
            _input_tokens: u32,
            _output_tokens: u32,
        ) -> f64 {
            0.0
        }

        async fn is_available(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "test"
        }

        fn models(&self) -> Vec<String> {
            vec![
                "qwen3-4b".into(),
                "codex-subscription".into(),
            ]
        }
    }

    #[test]
    fn available_models_include_subscription_models_once() {
        let mut router = ModelRouter::new();
        router.register_provider(Arc::new(TestProvider));

        let models = available_model_ids(&router);
        let mut sorted = models.clone();
        sorted.sort();

        assert_eq!(models, sorted);
        assert!(models.contains(&"qwen3-4b".to_string()));
        assert!(models.contains(
            &"claude-code-subscription".to_string()
        ));
        assert_eq!(
            models
                .iter()
                .filter(|model| {
                    model.as_str() == "codex-subscription"
                })
                .count(),
            1
        );
    }
}
