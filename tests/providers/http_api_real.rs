//! The HTTP API serving real API-key providers: openai, openrouter,
//! featherless.
//!
//! `POST /v1/chat/completions` is the surface every client actually uses, and
//! for an API-key provider its whole claim is: an authenticated request names
//! a canonical `provider/model` route, the gateway redeems the deployment's
//! own capability at final use, the real provider answers, and the response
//! comes back in the OpenAI-compatible envelope with real token usage. One
//! test per provider, over a real `brama serve` process -- no stub provider,
//! no dry run.
//!
//! What is real and what is test-owned is drawn deliberately. The gateway
//! process, the capability environment, the vault-held provider keys and the
//! providers themselves are the real ones -- these tests inherit the
//! launcher's environment and must run inside it:
//!
//! ```console
//! $ scripts/start-with-skarbiec.sh --exec cargo test --test http_api_real
//! ```
//!
//! Only the caller's side is test-owned: each serve gets one client identity
//! minted for the test (`BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES`), because the
//! bearer a caller presents is the caller's own credential and not the thing
//! under test, plus a private state/perf area so a test gateway's bookkeeping
//! never mixes into the serving one's. Every request here is a real billable
//! completion on the deployment's account, kept cheap with a small
//! `max_tokens`.
//!
//! `openrouter` is in this file on the operator's word: the vault holds the
//! `provider:openrouter` account. On a deployment whose launcher has not
//! configured the openrouter capability, that test fails with the gateway's
//! own `credential_unauthorized` sentence -- which is the finding, not a
//! reason to soften the test.

use std::io::Write as _;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const BEARER: &str = "brama-http-api-real-test-token";

/// The three canonical routes under test; models are pinned so a failure names
/// the exact route that failed instead of whatever a catalog offered that day.
const OPENAI_ROUTE: &str = "openai/default";
const OPENROUTER_ROUTE: &str = "openrouter/openai/gpt-4o-mini";
const FEATHERLESS_ROUTE: &str = "featherless/TheDrummer/Cydonia-24B-v4.3";

fn available_port() -> u16 {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .expect("reserve loopback port")
        .local_addr()
        .expect("reserved port")
        .port()
}

/// A private scratch directory under the operator-visible work area.
fn scratch(story: &str) -> PathBuf {
    let home = std::env::var("HOME").expect("HOME is required");
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let path = PathBuf::from(home)
        .join(".stado/work/brama-tests")
        .join(format!("{story}-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create test scratch directory");
    path
}

struct RealGateway {
    child: Child,
    origin: String,
    scratch: PathBuf,
}

impl RealGateway {
    /// The real binary serving with the real inherited environment. Only the
    /// client identity, the state directory and the perf file are test-owned.
    fn start(story: &str, allowed_route: &str) -> Self {
        let port = available_port();
        let scratch = scratch(story);
        let mut identities: Vec<Value> = serde_json::from_str(
            &std::env::var("BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES")
                .expect("the launcher must provide its real client identity table"),
        )
        .expect("the launcher client identity table must be JSON");
        identities.push(json!({
            "client_id": "http-api-real-test",
            "token": BEARER,
            "allowed_models": [allowed_route],
        }));
        let child = Command::new(env!("CARGO_BIN_EXE_brama"))
            .args(["serve", "--port", &port.to_string()])
            .env(
                "BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES",
                serde_json::to_string(&identities).expect("serialize client identity table"),
            )
            .env("BRAMA_STATE_DIR", scratch.join("state"))
            .env("BRAMA_PERF_PATH", scratch.join("perf.json"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start real Brama gateway");
        let origin = format!("http://127.0.0.1:{port}");
        Self {
            child,
            origin,
            scratch,
        }
    }

    fn wait_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("HTTP client");
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().expect("poll gateway") {
                let mut stderr = String::new();
                if let Some(mut pipe) = self.child.stderr.take() {
                    use std::io::Read as _;
                    let _ = pipe.read_to_string(&mut stderr);
                }
                panic!(
                    "the real gateway exited before binding ({status}); it must run inside the \
                     launcher environment: {stderr}"
                );
            }
            if client
                .get(format!("{}/health", self.origin))
                .send()
                .is_ok_and(|response| response.status().is_success())
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the real gateway did not bind its loopback endpoint in time");
    }
}

impl Drop for RealGateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}

/// One real completion through the HTTP surface for one provider route: 200,
/// the OpenAI-compatible envelope with real content and real token usage, and
/// the gateway's own perf record for the route as the product-recorded state.
fn api_serves_a_real_completion(story: &str, route: &str) {
    let mut gateway = RealGateway::start(story, route);
    gateway.wait_ready();

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("HTTP client");
    let response = client
        .post(format!("{}/v1/chat/completions", gateway.origin))
        .bearer_auth(BEARER)
        .json(&json!({
            "model": route,
            "messages": [{"role": "user", "content": "Say hello in one sentence."}],
            "max_tokens": 64,
            "temperature": 0.0,
        }))
        .send()
        .expect("gateway HTTP response");
    let status = response.status();
    let body: Value = response.json().expect("gateway response is JSON");
    assert!(
        status.is_success(),
        "the {route} route did not serve over the API: HTTP {status}: {body}"
    );

    // The response envelope, with a real answer inside it.
    let content = body
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !content.trim().is_empty(),
        "the {route} answer carries no content: {body}"
    );
    assert_eq!(
        body.pointer("/choices/0/message/role")
            .and_then(Value::as_str),
        Some("assistant"),
        "{body}"
    );
    assert!(
        body.pointer("/usage/total_tokens")
            .and_then(Value::as_u64)
            .is_some_and(|tokens| tokens > 0),
        "a real completion states real token usage: {body}"
    );
    assert!(
        body.pointer("/model")
            .and_then(Value::as_str)
            .is_some_and(|model| !model.is_empty()),
        "{body}"
    );

    // The product-recorded state: the gateway's own perf ledger holds the
    // served request. Flushed by the serving process, so give it a moment.
    let perf_path = gateway.scratch.join("perf.json");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut recorded = String::new();
    while Instant::now() < deadline {
        recorded = std::fs::read_to_string(&perf_path).unwrap_or_default();
        if !recorded.trim().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !recorded.trim().is_empty(),
        "the gateway recorded nothing in its perf ledger for the served request"
    );

    // Keep the transcript honest in a failure report: the answer itself.
    let mut sink = std::io::stdout();
    let _ = writeln!(sink, "{route} answered: {content}");
}

#[test]
fn the_api_serves_a_real_openai_completion() {
    api_serves_a_real_completion("api-openai", OPENAI_ROUTE);
}

#[test]
fn the_api_serves_a_real_openrouter_completion() {
    api_serves_a_real_completion("api-openrouter", OPENROUTER_ROUTE);
}

#[test]
fn the_api_serves_a_real_featherless_completion() {
    api_serves_a_real_completion("api-featherless", FEATHERLESS_ROUTE);
}
