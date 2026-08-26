#[path = "../support/mod.rs"]
mod support;

use reqwest::Method;
use serde_json::json;
use support::{start_provider, BramaProcess, TestDirectory, CLIENT_TOKEN, DESKTOP_TOKEN};

async fn install_openai_key(brama: &BramaProcess) {
    let (status, body) = brama
        .admin(
            Method::PUT,
            "/v1/admin/credentials",
            Some(json!({"provider":"openai","credential":"surface-key"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
}

#[tokio::test]
async fn every_inference_format_reaches_the_real_gateway_and_provider() {
    let directory = TestDirectory::new("inference-formats");
    let (provider_origin, provider, provider_task) = start_provider().await;
    let brama = BramaProcess::start(&directory, &provider_origin).await;
    install_openai_key(&brama).await;

    let (status, chat) = brama
        .request(
            Method::POST,
            "/v1/chat/completions",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"openai/default","messages":[{"role":"user","content":"chat"}]})),
        )
        .await;
    assert_eq!(status, 200, "{chat}");
    assert_eq!(chat["object"], "chat.completion");
    assert_eq!(
        chat["choices"][0]["message"]["content"],
        "hello from provider"
    );

    let (status, anthropic) = brama
        .request(
            Method::POST,
            "/v1/messages",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"openai/default","max_tokens":32,"messages":[{"role":"user","content":"messages"}]})),
        )
        .await;
    assert_eq!(status, 200, "{anthropic}");
    assert_eq!(anthropic["type"], "message");
    assert_eq!(anthropic["content"][0]["text"], "hello from provider");

    let (status, responses) = brama
        .request(
            Method::POST,
            "/v1/responses",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"openai/default","input":"responses"})),
        )
        .await;
    assert_eq!(status, 200, "{responses}");
    assert_eq!(responses["object"], "response");
    assert_eq!(
        responses["output"][0]["content"][0]["text"],
        "hello from provider"
    );

    let (status, chat_stream) = brama
        .request_text(
            Method::POST,
            "/v1/chat/completions",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"openai/default","stream":true,"messages":[{"role":"user","content":"stream chat"}]})),
        )
        .await;
    assert_eq!(status, 200, "{chat_stream}");
    assert!(
        chat_stream.contains("chat.completion.chunk"),
        "{chat_stream}"
    );
    assert!(chat_stream.contains("data: [DONE]"), "{chat_stream}");

    let (status, message_stream) = brama
        .request_text(
            Method::POST,
            "/v1/messages",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"openai/default","max_tokens":32,"stream":true,"messages":[{"role":"user","content":"stream messages"}]})),
        )
        .await;
    assert_eq!(status, 200, "{message_stream}");
    assert!(
        message_stream.contains("event: message_start"),
        "{message_stream}"
    );
    assert!(
        message_stream.contains("event: message_stop"),
        "{message_stream}"
    );

    let (status, response_stream) = brama
        .request_text(
            Method::POST,
            "/v1/responses",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"openai/default","input":"stream responses","stream":true})),
        )
        .await;
    assert_eq!(status, 200, "{response_stream}");
    assert!(
        response_stream.contains("event: response.created"),
        "{response_stream}"
    );
    assert!(
        response_stream.contains("event: response.completed"),
        "{response_stream}"
    );

    assert_eq!(
        provider
            .authorization
            .lock()
            .expect("authorization log")
            .as_slice(),
        [
            "Bearer surface-key",
            "Bearer surface-key",
            "Bearer surface-key",
            "Bearer surface-key",
            "Bearer surface-key",
            "Bearer surface-key",
        ]
    );
    provider_task.abort();
}

#[tokio::test]
async fn catalog_embeddings_moderations_health_readiness_and_stats_report_real_state() {
    let directory = TestDirectory::new("typed-and-operations");
    let (provider_origin, _provider, provider_task) = start_provider().await;
    let brama = BramaProcess::start(&directory, &provider_origin).await;
    install_openai_key(&brama).await;

    let (status, health) = brama.request(Method::GET, "/health", None, None).await;
    assert_eq!(status, 200, "{health}");
    assert_eq!(health["status"], "ok");
    assert_eq!(health["dependencies"], "not_probed");

    let (status, readiness) = brama.request(Method::GET, "/readyz", None, None).await;
    assert_eq!(status, 200, "{readiness}");
    assert_eq!(readiness["ready"], true);
    assert_eq!(readiness["providers"][0]["provider"], "openai");
    assert_eq!(readiness["providers"][0]["credential"], true);

    let (status, models) = brama
        .request(Method::GET, "/v1/models", Some(CLIENT_TOKEN), None)
        .await;
    assert_eq!(status, 200, "{models}");
    assert!(
        models["data"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty()),
        "{models}"
    );

    let (status, embedding) = brama
        .request(
            Method::POST,
            "/v1/embeddings",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"wisent-backend/embeddings","input":"embed this"})),
        )
        .await;
    assert_eq!(status, 200, "{embedding}");
    assert_eq!(embedding["data"][0]["embedding"], json!([0.25, 0.75]));

    let (status, moderation) = brama
        .request(
            Method::POST,
            "/v1/moderations",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"wisent-backend/moderation","input":"moderate this"})),
        )
        .await;
    assert_eq!(status, 200, "{moderation}");
    assert_eq!(moderation["results"][0]["flagged"], false);

    let (status, denied) = brama.request(Method::GET, "/stats", None, None).await;
    assert_eq!(status, 401, "{denied}");
    let (status, stats) = brama
        .request(Method::GET, "/stats", Some(DESKTOP_TOKEN), None)
        .await;
    assert_eq!(status, 200, "{stats}");
    assert!(
        stats["total_requests"]
            .as_u64()
            .is_some_and(|count| count >= 2),
        "{stats}"
    );
    assert_eq!(stats["configuredDirectProviders"], 1);

    provider_task.abort();
}

#[tokio::test]
async fn authentication_allowlists_and_invalid_requests_fail_closed() {
    let directory = TestDirectory::new("http-refusals");
    let (provider_origin, _provider, provider_task) = start_provider().await;
    let brama = BramaProcess::start(&directory, &provider_origin).await;
    install_openai_key(&brama).await;

    let request = json!({"model":"openai/default","messages":[{"role":"user","content":"deny"}]});
    let (status, missing) = brama
        .request(
            Method::POST,
            "/v1/chat/completions",
            None,
            Some(request.clone()),
        )
        .await;
    assert_eq!(status, 401, "{missing}");
    assert_eq!(missing["error"]["code"], "unauthenticated");

    let (status, wrong) = brama
        .request(
            Method::POST,
            "/v1/chat/completions",
            Some("wrong-token"),
            Some(request),
        )
        .await;
    assert_eq!(status, 401, "{wrong}");

    let (status, forbidden) = brama
        .request(
            Method::POST,
            "/v1/chat/completions",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"anthropic/claude-sonnet-4-6","messages":[{"role":"user","content":"deny"}]})),
        )
        .await;
    assert_eq!(status, 403, "{forbidden}");
    assert_eq!(forbidden["error"]["code"], "forbidden");

    let (status, invalid) = brama
        .request(
            Method::POST,
            "/v1/chat/completions",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"not-a-route","messages":[{"role":"user","content":"deny"}]})),
        )
        .await;
    assert_eq!(status, 403, "{invalid}");
    assert_eq!(invalid["error"]["code"], "forbidden");

    provider_task.abort();
}
