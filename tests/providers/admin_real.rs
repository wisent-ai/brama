//! Real administration lifecycles against the real Brama binary, Skarbiec,
//! and OpenRouter. No provider replacement, canned response, or dry run.

use std::io::Write as _;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use serde_json::{json, Value};

const DESKTOP_BEARER: &str = "brama-admin-real-desktop";
const CLIENT_BEARER: &str = "brama-admin-real-client";
const ROUTE: &str = "openrouter/openai/gpt-4o-mini";
const ALIAS: &str = "qualification/admin-real";
const AGENT: &str = "brama-real-qualification";

fn real_provider_credential(provider: &str) -> String {
    let resource = format!("provider:{provider}");
    let routes_path = std::env::var("SKARBIEC_CAPABILITY_ROUTES_FILE")
        .expect("the launcher must provide SKARBIEC_CAPABILITY_ROUTES_FILE");
    let routes: Value = serde_json::from_slice(
        &std::fs::read(routes_path).expect("read the real capability route table"),
    )
    .expect("capability route table is JSON");
    let entry = routes
        .get("routes")
        .unwrap_or(&routes)
        .get(&resource)
        .unwrap_or_else(|| panic!("the real route table has no {resource}"));
    let item = entry["item"].as_str().expect("route item");
    let field = entry["field"].as_str().expect("route field");
    let router = std::env::var("ENTITLEMENTS_ROUTER_BIN")
        .expect("the launcher must provide ENTITLEMENTS_ROUTER_BIN");
    let output = Command::new(router)
        .args(["get", item])
        .output()
        .expect("read the real provider item through Skarbiec");
    assert!(
        output.status.success(),
        "Skarbiec refused the real provider item: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).expect("provider item is JSON");
    document["fields"][field]
        .as_str()
        .filter(|value| !value.is_empty())
        .expect("the routed provider field contains a credential")
        .to_owned()
}

fn available_port() -> u16 {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .expect("reserve loopback port")
        .local_addr()
        .expect("reserved port")
        .port()
}

fn scratch(story: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let path = PathBuf::from(std::env::var("HOME").expect("HOME"))
        .join(".stado/work/brama-tests")
        .join(format!("{story}-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create qualification state");
    path
}

struct Gateway {
    child: Child,
    origin: String,
    client: Client,
    scratch: PathBuf,
}

impl Gateway {
    fn start() -> Self {
        let scratch = scratch("admin-real");
        let routes = scratch.join("routes.json");
        std::fs::write(&routes, br#"{"deployments":[],"routes":{},"fallbacks":{}}"#)
            .expect("create private routes file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&routes, std::fs::Permissions::from_mode(0o600))
                .expect("protect routes file");
        }

        let mut identities: Vec<Value> = serde_json::from_str(
            &std::env::var("BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES")
                .expect("the launcher must provide client identities"),
        )
        .expect("client identities are JSON");
        for identity in &mut identities {
            if identity["client_id"] == "brama-desktop" {
                identity["token"] = Value::String(DESKTOP_BEARER.into());
            }
        }
        identities.push(json!({
            "client_id": "brama-admin-real-client",
            "token": CLIENT_BEARER,
            "allowed_models": [ALIAS, ROUTE],
        }));

        let port = available_port();
        let mut child = Command::new(env!("CARGO_BIN_EXE_brama"))
            .args(["serve", "--port", &port.to_string(), "--local-credentials-stdin"])
            .env(
                "BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES",
                serde_json::to_string(&identities).expect("serialize identities"),
            )
            .env("BRAMA_INFERENCE_ROUTES_FILE", &routes)
            .env("BRAMA_STATE_DIR", scratch.join("state"))
            .env("BRAMA_PERF_PATH", scratch.join("perf.json"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the real Brama binary");
        child
            .stdin
            .take()
            .expect("credential stdin")
            .write_all(b"{}")
            .expect("start with an empty standalone credential store");

        let origin = format!("http://127.0.0.1:{port}");
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("HTTP client");
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if client
                .get(format!("{origin}/health"))
                .send()
                .is_ok_and(|response| response.status().is_success())
            {
                return Self { child, origin, client, scratch };
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the real Brama binary did not bind");
    }

    fn request(&self, method: reqwest::Method, path: &str, bearer: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = self.client.request(method, format!("{}{}", self.origin, path)).bearer_auth(bearer);
        if let Some(body) = body { request = request.json(&body); }
        let response = request.send().expect("Brama response");
        let status = response.status().as_u16();
        let body = response.json().expect("Brama JSON response");
        (status, body)
    }

    fn admin(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        self.request(method, path, DESKTOP_BEARER, body)
    }


    fn request_text(
        &self,
        method: reqwest::Method,
        path: &str,
        bearer: &str,
        body: Value,
    ) -> (u16, String) {
        let response = self
            .client
            .request(method, format!("{}{}", self.origin, path))
            .bearer_auth(bearer)
            .json(&body)
            .send()
            .expect("Brama streaming response");
        let status = response.status().as_u16();
        let body = response.text().expect("Brama streaming body");
        (status, body)
    }
    fn completion(&self, model: &str) -> (u16, Value) {
        self.request(
            reqwest::Method::POST,
            "/v1/chat/completions",
            CLIENT_BEARER,
            Some(json!({
                "model": model,
                "messages": [{"role":"user","content":"Answer with one short sentence."}],
                "max_tokens": 32,
                "temperature": 0.0
            })),
        )
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

#[test]
fn alias_add_edit_and_delete_changes_real_openrouter_dispatch() {
    let gateway = Gateway::start();
    let credential = real_provider_credential("openrouter");
    let (status, installed) = gateway.admin(
        reqwest::Method::PUT,
        "/v1/admin/credentials",
        Some(json!({"provider":"openrouter","credential":credential})),
    );
    assert_eq!(status, 200, "{installed}");
    let (status, created) = gateway.admin(
        reqwest::Method::PUT,
        "/v1/admin/routes",
        Some(json!({"alias":ALIAS,"primary":ROUTE,"fallbacks":[]})),
    );
    assert_eq!(status, 200, "{created}");
    let (status, answer) = gateway.completion(ALIAS);
    assert_eq!(status, 200, "{answer}");
    assert!(answer.pointer("/choices/0/message/content").and_then(Value::as_str).is_some_and(|text| !text.trim().is_empty()));

    let (status, edited) = gateway.admin(
        reqwest::Method::PUT,
        "/v1/admin/routes",
        Some(json!({"alias":ALIAS,"primary":ROUTE,"fallbacks":["openrouter/google/gemini-2.0-flash-001"]})),
    );
    assert_eq!(status, 200, "{edited}");
    assert_eq!(
        edited["routes"]["fallbacks"][ALIAS],
        json!(["openrouter/google/gemini-2.0-flash-001"])
    );
    let (status, answer) = gateway.completion(ALIAS);
    assert_eq!(status, 200, "{answer}");

    let (status, deleted) = gateway.admin(
        reqwest::Method::DELETE,
        "/v1/admin/routes",
        Some(json!({"alias":ALIAS})),
    );
    assert_eq!(status, 200, "{deleted}");
    let (status, _) = gateway.completion(ALIAS);
    assert_ne!(status, 200);
}

#[test]
fn key_add_replace_and_delete_changes_real_openrouter_dispatch() {
    let credential = real_provider_credential("openrouter");
    let gateway = Gateway::start();
    let (status, created) = gateway.admin(
        reqwest::Method::PUT,
        "/v1/admin/credentials",
        Some(json!({"provider":"openrouter","credential":credential})),
    );
    assert_eq!(status, 200, "{created}");
    let (status, answer) = gateway.completion(ROUTE);
    assert_eq!(status, 200, "{answer}");

    let credential = real_provider_credential("openrouter");
    let (status, replaced) = gateway.admin(
        reqwest::Method::PUT,
        "/v1/admin/credentials",
        Some(json!({"provider":"openrouter","credential":credential})),
    );
    assert_eq!(status, 200, "{replaced}");
    let (status, answer) = gateway.completion(ROUTE);
    assert_eq!(status, 200, "{answer}");

    let (status, deleted) = gateway.admin(
        reqwest::Method::DELETE,
        "/v1/admin/credentials",
        Some(json!({"provider":"openrouter"})),
    );
    assert_eq!(status, 200, "{deleted}");
    let (status, _) = gateway.completion(ROUTE);
    assert_ne!(status, 200);
}

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
        let (status, answer) = gateway.request(
            reqwest::Method::POST,
            path,
            CLIENT_BEARER,
            Some(body),
        );
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

    let (status, models) =
        gateway.request(reqwest::Method::GET, "/v1/models", CLIENT_BEARER, None);
    assert_eq!(status, 200, "{models}");
    assert!(models["data"].as_array().is_some_and(|rows| !rows.is_empty()));
    let (status, readiness) =
        gateway.request(reqwest::Method::GET, "/readyz", CLIENT_BEARER, None);
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

#[test]
fn subscription_add_replace_probe_and_delete_uses_real_openrouter_account() {
    let credential = real_provider_credential("openrouter");
    let gateway = Gateway::start();
    let collection = format!("/v1/admin/subscriptions/{AGENT}");
    let (status, created) = gateway.admin(
        reqwest::Method::POST,
        &collection,
        Some(json!({"provider":"openrouter","label":"primary","api_key":credential})),
    );
    assert_eq!(status, 200, "{created}");
    let id = created["subscription"]["id"].as_str().expect("subscription id").to_owned();
    let probe = format!("{collection}/{id}/probe");
    let (status, proved) = gateway.admin(reqwest::Method::POST, &probe, None);
    assert_eq!(status, 200, "{proved}");
    assert_eq!(proved["ok"], true);

    let credential = real_provider_credential("openrouter");
    let (status, replaced) = gateway.admin(
        reqwest::Method::POST,
        &collection,
        Some(json!({"provider":"openrouter","label":"replacement","api_key":credential})),
    );
    assert_eq!(status, 200, "{replaced}");
    assert_eq!(replaced["subscription"]["id"], id);
    assert_eq!(replaced["subscription"]["label"], "replacement");
    let (status, proved) = gateway.admin(reqwest::Method::POST, &probe, None);
    assert_eq!(status, 200, "{proved}");

    let item = format!("{collection}/{id}");
    let (status, deleted) = gateway.admin(reqwest::Method::DELETE, &item, None);
    assert_eq!(status, 200, "{deleted}");
    let (status, missing) = gateway.admin(reqwest::Method::POST, &probe, None);
    assert_eq!(status, 404, "{missing}");
}
