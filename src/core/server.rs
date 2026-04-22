use std::sync::atomic::Ordering;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tracing::info;

use super::router::ModelRouter;
use crate::types::{Message, Tool, ToolCall};

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
    content: Option<String>,
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
    Json(req): Json<ChatCompletionRequest>,
) -> impl IntoResponse {
    let messages: Vec<Message> = req
        .messages
        .into_iter()
        .map(|m| Message {
            role: m.role,
            content: m.content.unwrap_or_default(),
            tool_call_id: m.tool_call_id,
            name: m.name,
            tool_calls: m.tool_calls,
        })
        .collect();

    let r = router.read().await;
    let resp = r
        .complete(
            messages,
            &req.model,
            req.max_tokens,
            req.temperature,
            None,
            req.tools,
        )
        .await;

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

async fn list_models(
    State(router): State<SharedRouter>,
) -> impl IntoResponse {
    let r = router.read().await;
    let models: Vec<Value> = r
        .all_models()
        .into_iter()
        .map(|id| {
            json!({
                "id": id,
                "object": "model",
                "owned_by": "model-router",
            })
        })
        .collect();

    Json(json!({
        "object": "list",
        "data": models,
    }))
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
        .route("/health", get(health))
        .route("/stats", get(get_stats))
        .with_state(shared);

    let gateway_app = Router::new()
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
