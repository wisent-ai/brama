#[path = "../support/mod.rs"]
mod support;

use reqwest::Method;
use serde_json::json;
use support::{start_provider, BramaProcess, TestDirectory};

#[tokio::test]
async fn add_replace_probe_and_delete_subscription_uses_real_provider_path() {
    let directory = TestDirectory::new("subscription-lifecycle");
    let (provider_origin, provider, provider_task) = start_provider().await;
    let brama = BramaProcess::start(&directory, &provider_origin).await;
    let collection = "/v1/admin/subscriptions/contract-agent";

    let (status, body) = brama.admin(Method::GET, collection, None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["subscriptions"], json!([]));

    let (status, created) = brama
        .admin(
            Method::POST,
            collection,
            Some(json!({"provider":"anthropic","label":"primary","api_key":"subscription-one"})),
        )
        .await;
    assert_eq!(status, 200, "{created}");
    let subscription_id = created["subscription"]["id"]
        .as_str()
        .expect("created subscription id")
        .to_owned();
    assert_eq!(
        subscription_id,
        "brama-sub-contract-agent-anthropic-primary"
    );
    assert_eq!(created["subscription"]["label"], "primary");

    let probe_path = format!("{collection}/{subscription_id}/probe");
    let (status, probe) = brama.admin(Method::POST, &probe_path, None).await;
    assert_eq!(status, 200, "{probe}");
    assert_eq!(probe["ok"], true);

    let (status, replaced) = brama
        .admin(
            Method::POST,
            collection,
            Some(
                json!({"provider":"anthropic","label":"replacement","api_key":"subscription-two"}),
            ),
        )
        .await;
    assert_eq!(status, 200, "{replaced}");
    assert_eq!(replaced["subscription"]["id"], subscription_id);
    assert_eq!(replaced["subscription"]["label"], "replacement");
    let (status, listed) = brama.admin(Method::GET, collection, None).await;
    assert_eq!(status, 200, "{listed}");
    let subscriptions = listed["subscriptions"].as_array().expect("subscriptions");
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0]["label"], "replacement");

    let (status, probe) = brama.admin(Method::POST, &probe_path, None).await;
    assert_eq!(status, 200, "{probe}");
    assert_eq!(
        provider
            .authorization
            .lock()
            .expect("authorization log")
            .as_slice(),
        ["subscription-one", "subscription-two"]
    );

    let item_path = format!("{collection}/{subscription_id}");
    let (status, body) = brama.admin(Method::DELETE, &item_path, None).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["ok"], true);
    let (status, listed) = brama.admin(Method::GET, collection, None).await;
    assert_eq!(status, 200, "{listed}");
    assert_eq!(listed["subscriptions"], json!([]));

    let (status, body) = brama.admin(Method::POST, &probe_path, None).await;
    assert_eq!(status, 404, "{body}");
    let (status, body) = brama.admin(Method::DELETE, &item_path, None).await;
    assert_eq!(status, 404, "{body}");
    provider_task.abort();
}
