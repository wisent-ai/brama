#[path = "../support/mod.rs"]
mod support;

use reqwest::Method;
use serde_json::json;
use support::{start_provider, BramaProcess, TestDirectory, CLIENT_TOKEN};

#[tokio::test]
async fn add_edit_and_delete_alias_changes_real_routing_registry() {
    let directory = TestDirectory::new("alias-lifecycle");
    let (provider_origin, provider, provider_task) = start_provider().await;
    let brama = BramaProcess::start(&directory, &provider_origin).await;
    let (status, body) = brama
        .admin(
            Method::PUT,
            "/v1/admin/credentials",
            Some(json!({"provider":"openai","credential":"alias-key"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");

    let (status, body) = brama
        .admin(
            Method::PUT,
            "/v1/admin/routes",
            Some(json!({"alias":"qa/chat","primary":"openai/default","fallbacks":[]})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["routes"]["routes"]["qa/chat"], "openai/default");

    let (status, body) = brama
        .request(
            Method::POST,
            "/v1/chat/completions",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"qa/chat","messages":[{"role":"user","content":"route me"}]})),
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
            "/v1/admin/routes",
            Some(json!({
                "alias":"qa/chat",
                "primary":"openai/fail",
                "fallbacks":["openai/default"]
            })),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["routes"]["routes"]["qa/chat"], "openai/fail");
    assert_eq!(
        body["routes"]["fallbacks"]["qa/chat"],
        json!(["openai/default"])
    );

    let (status, snapshot) = brama.admin(Method::GET, "/v1/admin/snapshot", None).await;
    assert_eq!(status, 200, "{snapshot}");
    assert_eq!(snapshot["routes"]["routes"]["qa/chat"], "openai/fail");

    let (status, body) = brama
        .request(
            Method::POST,
            "/v1/chat/completions",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"qa/chat","messages":[{"role":"user","content":"use fallback"}]})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "hello from provider"
    );
    assert_eq!(
        provider
            .authorization
            .lock()
            .expect("authorization log")
            .as_slice(),
        ["Bearer alias-key", "Bearer alias-key", "Bearer alias-key"]
    );

    let (status, body) = brama
        .admin(
            Method::DELETE,
            "/v1/admin/routes",
            Some(json!({"alias":"qa/chat"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["routes"]["routes"].get("qa/chat").is_none());
    assert!(body["routes"]["fallbacks"].get("qa/chat").is_none());

    let (status, body) = brama
        .request(
            Method::POST,
            "/v1/chat/completions",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"qa/chat","messages":[{"role":"user","content":"gone"}]})),
        )
        .await;
    assert_eq!(status, 503, "{body}");

    let (status, body) = brama
        .admin(
            Method::DELETE,
            "/v1/admin/routes",
            Some(json!({"alias":"qa/chat"})),
        )
        .await;
    assert_eq!(status, 404, "{body}");

    let (status, body) = brama
        .admin(
            Method::DELETE,
            "/v1/admin/routes",
            Some(json!({"alias":"best"})),
        )
        .await;
    assert_eq!(status, 409, "{body}");
    provider_task.abort();
}
