#[path = "../support/mod.rs"]
mod support;

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::process::{Command, Stdio};

use reqwest::Method;
use serde_json::json;
use support::{start_provider, BramaProcess, TestDirectory, CLIENT_TOKEN, DESKTOP_TOKEN};

#[tokio::test]
async fn deployment_owned_alias_reaches_its_declared_loopback_model() {
    let directory = TestDirectory::new("deployment-route");
    let (provider_origin, provider, provider_task) = start_provider().await;
    let endpoint = reqwest::Url::parse(&provider_origin).expect("provider URL");
    let registry = json!({
        "deployments": [{
            "name": "contract-local-model",
            "adapters": [],
            "endpoint": {
                "host": endpoint.host_str().expect("provider host"),
                "port": endpoint.port().expect("provider port")
            }
        }],
        "routes": {"qa/chat": "contract-local-model"},
        "fallbacks": {}
    });
    let bytes = serde_json::to_vec(&registry).expect("route registry");
    let brama = BramaProcess::start_with_registry(
        &directory,
        &provider_origin,
        &bytes,
        r#"{"local-openai":"local-key"}"#,
    )
    .await;

    let (status, body) = brama
        .request(
            Method::POST,
            "/v1/chat/completions",
            Some(CLIENT_TOKEN),
            Some(json!({"model":"qa/chat","messages":[{"role":"user","content":"local route"}]})),
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
        ["Bearer local-key"]
    );
    provider_task.abort();
}

#[test]
fn route_registry_rejects_insecure_permissions_and_non_mesh_endpoints() {
    let directory = TestDirectory::new("route-registry-refusals");
    let insecure_permissions = directory.path().join("group-readable.json");
    std::fs::write(
        &insecure_permissions,
        br#"{"deployments":[],"routes":{},"fallbacks":{}}"#,
    )
    .expect("write insecure fixture");
    std::fs::set_permissions(
        &insecure_permissions,
        std::fs::Permissions::from_mode(0o644),
    )
    .expect("set insecure permissions");
    let output = run_refused_server(&directory, &insecure_permissions);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("group or other"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let unsafe_endpoint = directory.path().join("unsafe-endpoint.json");
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true).mode(0o600);
    let mut file = options
        .open(&unsafe_endpoint)
        .expect("write unsafe endpoint fixture");
    file.write_all(
        br#"{"deployments":[{"name":"private-lan","adapters":[],"endpoint":{"host":"192.168.1.20","port":8000}}],"routes":{"qa/chat":"private-lan"},"fallbacks":{}}"#,
    )
    .expect("write unsafe endpoint");
    drop(file);
    let output = run_refused_server(&directory, &unsafe_endpoint);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("safe local or Tailscale endpoint"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_refused_server(directory: &TestDirectory, routes: &std::path::Path) -> std::process::Output {
    let identities = json!([{"client_id":"brama-desktop","token":DESKTOP_TOKEN}]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_brama"))
        .args(["serve", "--port", "0", "--local-credentials-stdin"])
        .env(
            "BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES",
            identities.to_string(),
        )
        .env("BRAMA_INFERENCE_ROUTES_FILE", routes)
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
        .expect("start Brama refusal case");
    child
        .stdin
        .take()
        .expect("Brama credential stdin")
        .write_all(b"{}")
        .expect("send empty credentials");
    child.wait_with_output().expect("Brama refusal output")
}
