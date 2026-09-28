//! `brama stub-provider`: a loopback OpenAI-shaped provider for the
//! documentation walk-through, so a standalone gateway can be tried with no
//! credential and no provider spend. It answers three models: `stub-ok`
//! completes (streamed or not), `stub-401` refuses the key, `stub-429`
//! rate-limits and names [`RETRY_AFTER_SECONDS`].

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Args;
use serde_json::{json, Value};

#[derive(Args)]
pub(crate) struct StubArgs {
    /// Loopback port to listen on
    #[arg(long, default_value_t = STUB_PORT)]
    port: u16,
}

/// The port `docs/examples/standalone-serve-with-stub.sh` points the gateway at.
const STUB_PORT: u16 = 18999;
const COMPLETION_ID: &str = "chatcmpl-stub-0001";
/// Every stub answer carries exactly one choice, the first.
const CHOICE_INDEX: u32 = 0;
/// The wait the rate-limited model asks the caller for.
const RETRY_AFTER_SECONDS: &str = "1";
/// The prompt and completion token counts the stub reports.
const USAGE: (u64, u64) = (9, 7);

fn json_response(status: StatusCode, body: &Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("static response")
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

async fn models() -> Response {
    json_response(StatusCode::OK, &json!({ "data": [{ "id": "stub-ok" }, { "id": "stub-401" }, { "id": "stub-429" }] }))
}

fn chunk(model: &str, created: u64, delta: Value, finish: Value) -> Value {
    json!({
        "id": COMPLETION_ID,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [{ "index": CHOICE_INDEX, "delta": delta, "finish_reason": finish }],
    })
}

async fn completion(Json(request): Json<Value>) -> Response {
    let model = request.get("model").and_then(Value::as_str).unwrap_or("").to_string();
    if model == "stub-401" {
        return json_response(
            StatusCode::UNAUTHORIZED,
            &json!({ "error": { "message": "Incorrect API key provided: sk-brama-docs-invalid.", "type": "invalid_request_error", "code": "invalid_api_key" } }),
        );
    }
    if model == "stub-429" {
        let mut response = json_response(
            StatusCode::TOO_MANY_REQUESTS,
            &json!({ "error": { "message": "Rate limit reached for stub-429.", "type": "tokens", "code": "rate_limit_exceeded" } }),
        );
        response.headers_mut().insert(header::RETRY_AFTER, RETRY_AFTER_SECONDS.parse().expect("static header"));
        return response;
    }
    let created = now();
    let (prompt, completion_tokens) = USAGE;
    let usage = json!({ "prompt_tokens": prompt, "completion_tokens": completion_tokens, "total_tokens": prompt + completion_tokens });
    if request.get("stream").and_then(Value::as_bool).unwrap_or(false) {
        let mut body = String::new();
        for piece in ["Hello ", "from ", "the stub."] {
            let part = chunk(&model, created, json!({ "content": piece }), Value::Null);
            body.push_str(&format!("data: {part}\n\n"));
        }
        let mut done = chunk(&model, created, json!({}), json!("stop"));
        done["usage"] = usage;
        body.push_str(&format!("data: {done}\n\ndata: [DONE]\n\n"));
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from(body))
            .expect("static response");
    }
    let message = json!({ "role": "assistant", "content": "Hello from the stub provider." });
    json_response(
        StatusCode::OK,
        &json!({
            "id": COMPLETION_ID,
            "object": "chat.completion",
            "created": created,
            "model": model,
            "choices": [{ "index": CHOICE_INDEX, "message": message, "finish_reason": "stop" }],
            "usage": usage,
        }),
    )
}

pub(crate) async fn serve(args: StubArgs) {
    let app = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/chat/completions", post(completion));
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", args.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("stub provider cannot listen on 127.0.0.1:{}: {error}", args.port);
            std::process::exit(1);
        }
    };
    eprintln!("stub provider on http://127.0.0.1:{}", args.port);
    if let Err(error) = axum::serve(listener, app).await {
        eprintln!("stub provider stopped: {error}");
        std::process::exit(1);
    }
}
