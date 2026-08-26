#[path = "../support/mod.rs"]
mod support;

use reqwest::Method;
use serde_json::json;
use support::{start_provider, BramaProcess, TestDirectory, CLIENT_TOKEN};

#[tokio::test]
async fn add_replace_and_delete_provider_key_changes_real_dispatch() {
    let directory = TestDirectory::new("credential-lifecycle");
    let (provider_origin, provider, provider_task) = start_provider().await;
    let brama = BramaProcess::start(&directory, &provider_origin).await;

    let (status, body) = brama
        .admin(Method::GET, "/v1/admin/credentials", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"providers": []}));

    let (status, body) = brama
        .admin(
            Method::PUT,
            "/v1/admin/credentials",
            Some(json!({"provider":"openai","credential":"first-key"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");

    let (status, body) = brama
        .request(
            Method::POST,
            "/v1/chat/completions",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"openai/default","messages":[{"role":"user","content":"hello"}]})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "hello from provider"
    );

    let (status, body) = brama
        .admin(
            Method::PUT,
            "/v1/admin/credentials",
            Some(json!({"provider":"openai","credential":"replacement-key"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = brama
        .request(
            Method::POST,
            "/v1/chat/completions",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"openai/default","messages":[{"role":"user","content":"hello again"}]})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        provider
            .authorization
            .lock()
            .expect("authorization log")
            .as_slice(),
        ["Bearer first-key", "Bearer replacement-key"]
    );

    let (status, body) = brama
        .admin(
            Method::DELETE,
            "/v1/admin/credentials",
            Some(json!({"provider":"openai"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = brama
        .admin(Method::GET, "/v1/admin/credentials", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"providers": []}));

    let (status, body) = brama
        .request(
            Method::POST,
            "/v1/chat/completions",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"openai/default","messages":[{"role":"user","content":"must fail"}]})),
        )
        .await;
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["error"]["code"], "dependency_unavailable");

    let (status, body) = brama
        .admin(
            Method::DELETE,
            "/v1/admin/credentials",
            Some(json!({"provider":"openai"})),
        )
        .await;
    assert_eq!(status, 404, "{body}");
    provider_task.abort();
}
