use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

pub const DESKTOP_TOKEN: &str = "brama-contract-desktop-token";
pub const CLIENT_TOKEN: &str = "brama-contract-client-token";

pub struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    pub fn new(story: &str) -> Self {
        let home = std::env::var_os("HOME").expect("HOME is required");
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let path = PathBuf::from(home)
            .join(".stado/work/brama-tests")
            .join(format!("{story}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).expect("create isolated Brama test directory");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub struct BramaProcess {
    child: Child,
    pub origin: String,
    pub client: reqwest::Client,
}

impl BramaProcess {
    pub async fn start(directory: &TestDirectory, provider_origin: &str) -> Self {
        Self::start_with_registry(
            directory,
            provider_origin,
            br#"{"deployments":[],"routes":{},"fallbacks":{}}"#,
            "{}",
        )
        .await
    }

    pub async fn start_with_registry(
        directory: &TestDirectory,
        provider_origin: &str,
        registry: &[u8],
        credentials: &str,
    ) -> Self {
        let port = available_port();
        let routes = directory.path().join("routes.json");
        write_owner_only(&routes, registry);
        let catalog = directory.path().join("models.json");
        write_owner_only(
            &catalog,
            br#"{"openai":{"id":"openai","name":"OpenAI","npm":"@ai-sdk/openai","models":{"default":{"id":"default","last_updated":"2026-08-25"},"gpt-5.4":{"id":"gpt-5.4","last_updated":"2026-08-25"},"embeddings":{"id":"embeddings","last_updated":"2026-08-25"},"moderation":{"id":"moderation","last_updated":"2026-08-25"}}}}"#,
        );
        let identities = json!([
            {"client_id":"brama-desktop","token":DESKTOP_TOKEN},
            {"client_id":"contract-client","token":CLIENT_TOKEN,"allowed_models":["qa/chat","qa/chat-edited","openai/default","wisent-backend/embeddings","wisent-backend/moderation"]}
        ]);
        let mut child = Command::new(env!("CARGO_BIN_EXE_brama"))
            .args([
                "serve",
                "--port",
                &port.to_string(),
                "--local-credentials-stdin",
            ])
            .env(
                "BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES",
                identities.to_string(),
            )
            .env("BRAMA_INFERENCE_ROUTES_FILE", &routes)
            .env(
                "BRAMA_MODEL_ALIASES",
                json!({
                    "wisent-backend/embeddings": "openai/embeddings",
                    "wisent-backend/moderation": "openai/moderation"
                })
                .to_string(),
            )
            .env("BRAMA_MODEL_CATALOG_PATH", &catalog)
            .env("BRAMA_PROVIDER_OPENAI_BASE_URL", provider_origin)
            .env("BRAMA_PROVIDER_ANTHROPIC_BASE_URL", provider_origin)
            .env("BRAMA_STATE_DIR", directory.path().join("state"))
            .env(
                "BRAMA_DONATED_SUBSCRIPTIONS_FILE",
                directory.path().join("subscriptions.json"),
            )
            .env("BRAMA_PERF_PATH", directory.path().join("perf.json"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start real Brama binary");
        child
            .stdin
            .take()
            .expect("Brama credential stdin")
            .write_all(credentials.as_bytes())
            .expect("send standalone credential store");
        let origin = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("HTTP client");
        for _ in 0..100 {
            if client
                .get(format!("{origin}/health"))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                return Self {
                    child,
                    origin,
                    client,
                };
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("real Brama binary did not bind its loopback endpoint");
    }

    pub async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = self
            .client
            .request(method, format!("{}{}", self.origin, path));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("Brama HTTP response");
        let status = response.status();
        let bytes = response.bytes().await.expect("Brama response body");
        let body = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({"raw": String::from_utf8_lossy(&bytes).into_owned()}));
        (status, body)
    }

    pub async fn request_text(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, String) {
        let mut request = self
            .client
            .request(method, format!("{}{}", self.origin, path));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.expect("Brama HTTP response");
        let status = response.status();
        let body = response.text().await.expect("Brama text response");
        (status, body)
    }

    pub async fn admin(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        self.request(method, path, Some(DESKTOP_TOKEN), body).await
    }
}

impl Drop for BramaProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Clone, Default)]
pub struct ProviderState {
    pub authorization: Arc<Mutex<Vec<String>>>,
}

pub async fn start_provider() -> (String, ProviderState, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind provider fixture");
    let address = listener.local_addr().expect("provider address");
    let state = ProviderState::default();
    let app = Router::new()
        .route("/v1/models", get(provider_models))
        .route("/v1/chat/completions", post(provider_chat))
        .route("/v1/messages", post(provider_anthropic))
        .route("/v1/embeddings", post(provider_embeddings))
        .route("/v1/moderations", post(provider_moderations))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve provider fixture");
    });
    (format!("http://{address}"), state, task)
}

async fn provider_models() -> Json<Value> {
    Json(json!({"object":"list","data":[{"id":"gpt-5.4","object":"model"}]}))
}

async fn provider_chat(
    State(state): State<ProviderState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    state.authorization.lock().expect("authorization log").push(
        headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
    );
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("gpt-5.4");
    if model == "fail" {
        return Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"error":{"message":"fixture primary failed"}}).to_string(),
            ))
            .expect("failure response");
    }
    if request.get("stream").and_then(Value::as_bool) == Some(true) {
        let stream = format!(
            "data: {{\"id\":\"chunk-1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"{model}\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"hello\"}},\"finish_reason\":null}}]}}\n\ndata: {{\"id\":\"chunk-1\",\"object\":\"chat.completion.chunk\",\"created\":1,\"model\":\"{model}\",\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n"
        );
        return Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(Body::from(stream))
            .expect("stream response");
    }
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "id":"chatcmpl-contract",
                "object":"chat.completion",
                "created":1,
                "model":model,
                "choices":[{"index":0,"message":{"role":"assistant","content":"hello from provider"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":3,"completion_tokens":4,"total_tokens":7}
            })
            .to_string(),
        ))
        .expect("chat response")
}

async fn provider_anthropic(
    State(state): State<ProviderState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Json<Value> {
    state.authorization.lock().expect("authorization log").push(
        headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned(),
    );
    Json(json!({
        "id": "msg-contract",
        "type": "message",
        "role": "assistant",
        "model": request.get("model").and_then(Value::as_str).unwrap_or("claude-haiku-4-5"),
        "content": [{"type": "text", "text": "hello from provider"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 1, "output_tokens": 1}
    }))
}

async fn provider_embeddings() -> Json<Value> {
    Json(
        json!({"object":"list","data":[{"object":"embedding","index":0,"embedding":[0.25,0.75]}],"model":"text-embedding-3-small","usage":{"prompt_tokens":1,"total_tokens":1}}),
    )
}

async fn provider_moderations() -> Json<Value> {
    Json(
        json!({"id":"modr-contract","model":"omni-moderation-latest","results":[{"flagged":false,"categories":{},"category_scores":{}}]}),
    )
}

fn available_port() -> u16 {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .expect("reserve loopback port")
        .local_addr()
        .expect("reserved port")
        .port()
}

fn write_owner_only(path: &Path, bytes: &[u8]) {
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).expect("create owner-only fixture");
    file.write_all(bytes).expect("write fixture");
}
