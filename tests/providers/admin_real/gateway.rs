//! The real Brama binary, started for one administration story.
//!
//! The gateway under test is the released binary over a private, owner-only
//! route registry; provider credentials come from the real Skarbiec route
//! table through the real entitlements router. No provider replacement,
//! canned response, or dry run.

use std::io::Write as _;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use serde_json::{json, Value};

const DESKTOP_BEARER: &str = "brama-admin-real-desktop";
pub(crate) const CLIENT_BEARER: &str = "brama-admin-real-client";
pub(crate) const ROUTE: &str = "openrouter/openai/gpt-4o-mini";
pub(crate) const ALIAS: &str = "qualification/admin-real";

pub(crate) fn real_provider_credential(provider: &str) -> String {
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

pub(crate) struct Gateway {
    child: Child,
    origin: String,
    client: Client,
    scratch: PathBuf,
}

impl Gateway {
    pub(crate) fn start() -> Self {
        let scratch = scratch("admin-real");
        let routes = scratch.join("routes.json");
        std::fs::write(
            &routes,
            br#"{"schema_version":1,"deployments":[],"routes":{}}"#,
        )
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
            "allowed_models": [ALIAS, ROUTE, super::routes::REPLACEMENT_ROUTE],
        }));

        let port = available_port();
        let mut child = Command::new(env!("CARGO_BIN_EXE_brama"))
            .args([
                "serve",
                "--port",
                &port.to_string(),
                "--local-credentials-stdin",
            ])
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
                return Self {
                    child,
                    origin,
                    client,
                    scratch,
                };
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("the real Brama binary did not bind");
    }

    pub(crate) fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        bearer: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut request = self
            .client
            .request(method, format!("{}{}", self.origin, path))
            .bearer_auth(bearer);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().expect("Brama response");
        let status = response.status().as_u16();
        let body = response.json().expect("Brama JSON response");
        (status, body)
    }

    pub(crate) fn admin(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        self.request(method, path, DESKTOP_BEARER, body)
    }

    pub(crate) fn request_text(
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

    pub(crate) fn completion(&self, model: &str) -> (u16, Value) {
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
