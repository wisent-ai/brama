#[path = "../support/mod.rs"]
mod support;

use reqwest::Method;
use serde_json::json;
use support::{start_provider, BramaProcess, TestDirectory, CLIENT_TOKEN, DESKTOP_TOKEN};

#[tokio::test]
async fn successful_inference_updates_stats_and_survives_gateway_restart() {
    let directory = TestDirectory::new("telemetry-persistence");
    let (provider_origin, _provider, provider_task) = start_provider().await;

    {
        let brama = BramaProcess::start(&directory, &provider_origin).await;
        let (status, body) = brama
            .admin(
                Method::PUT,
                "/v1/admin/credentials",
                Some(json!({"provider":"openai","credential":"telemetry-key"})),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        let (status, body) = brama
            .request(
                Method::POST,
                "/v1/chat/completions",
                Some(CLIENT_TOKEN),
                Some(json!({"model":"openai/default","messages":[{"role":"user","content":"record telemetry"}]})),
            )
            .await;
        assert_eq!(status, 200, "{body}");
        let (status, stats) = brama
            .request(Method::GET, "/stats", Some(DESKTOP_TOKEN), None)
            .await;
        assert_eq!(status, 200, "{stats}");
        let row = stats["models"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["model"] == "openai/default"))
            .expect("telemetry row for routed model");
        assert_eq!(row["count"], 1);
    }

    let brama = BramaProcess::start(&directory, &provider_origin).await;
    let (status, stats) = brama
        .request(Method::GET, "/stats", Some(DESKTOP_TOKEN), None)
        .await;
    assert_eq!(status, 200, "{stats}");
    let row = stats["models"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["model"] == "openai/default"))
        .expect("persisted telemetry row");
    assert_eq!(row["count"], 1);
    assert!(directory.path().join("perf.json").is_file());

    provider_task.abort();
}
