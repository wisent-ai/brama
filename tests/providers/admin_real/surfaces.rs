//! Every chat surface and operational read, over one real credential.
//!
//! The three dialects are exercised buffered and streamed against real
//! OpenRouter, then the model listing, readiness and statistics are read from
//! the same run that produced them.

use serde_json::{json, Value};

use super::gateway::{real_provider_credential, Gateway, CLIENT_BEARER, ROUTE};

#[test]
fn every_chat_surface_and_operational_read_uses_real_openrouter_state() {
    let credential = real_provider_credential("openrouter");
    let gateway = Gateway::start();
    let (status, installed) = gateway.admin(
        reqwest::Method::PUT,
        "/v1/admin/credentials",
        Some(json!({"provider":"openrouter","credential":credential})),
    );
    assert_eq!(status, 200, "{installed}");

    for (path, body, pointer) in [
        (
            "/v1/chat/completions",
            json!({"model":ROUTE,"messages":[{"role":"user","content":"Reply briefly."}],"max_tokens":32}),
            "/choices/0/message/content",
        ),
        (
            "/v1/messages",
            json!({"model":ROUTE,"messages":[{"role":"user","content":"Reply briefly."}],"max_tokens":32}),
            "/content/0/text",
        ),
        (
            "/v1/responses",
            json!({"model":ROUTE,"input":"Reply briefly.","max_output_tokens":32}),
            "/output/0/content/0/text",
        ),
    ] {
        let (status, answer) =
            gateway.request(reqwest::Method::POST, path, CLIENT_BEARER, Some(body));
        assert_eq!(status, 200, "{path}: {answer}");
        assert!(
            answer
                .pointer(pointer)
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty()),
            "{path}: {answer}"
        );
    }

    for (path, body, terminal) in [
        (
            "/v1/chat/completions",
            json!({"model":ROUTE,"stream":true,"messages":[{"role":"user","content":"Reply briefly."}],"max_tokens":32}),
            "data: [DONE]",
        ),
        (
            "/v1/messages",
            json!({"model":ROUTE,"stream":true,"messages":[{"role":"user","content":"Reply briefly."}],"max_tokens":32}),
            "event: message_stop",
        ),
        (
            "/v1/responses",
            json!({"model":ROUTE,"stream":true,"input":"Reply briefly.","max_output_tokens":32}),
            "event: response.completed",
        ),
    ] {
        let (status, stream) =
            gateway.request_text(reqwest::Method::POST, path, CLIENT_BEARER, body);
        assert_eq!(status, 200, "{path}: {stream}");
        assert!(stream.contains(terminal), "{path}: {stream}");
    }

    let (status, models) = gateway.request(reqwest::Method::GET, "/v1/models", CLIENT_BEARER, None);
    assert_eq!(status, 200, "{models}");
    assert!(models["data"]
        .as_array()
        .is_some_and(|rows| !rows.is_empty()));
    let (status, readiness) = gateway.request(reqwest::Method::GET, "/readyz", CLIENT_BEARER, None);
    assert_eq!(status, 200, "{readiness}");
    assert_eq!(readiness["ready"], true);
    let (status, stats) = gateway.admin(reqwest::Method::GET, "/stats", None);
    assert_eq!(status, 200, "{stats}");
    assert!(
        stats["total_requests"]
            .as_u64()
            .is_some_and(|requests| requests >= 6),
        "{stats}"
    );
}
