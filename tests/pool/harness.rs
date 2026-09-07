//! One real gateway, one isolated vault, one isolated identity authority.
//!
//! Everything the capability itself does is real here: the released binary
//! serves real HTTP on loopback, resolves bearers through its own identity
//! path, verifies the HMAC trio with its own signing code, and writes its own
//! journal and usage ledger. What is isolated is the two authorities behind
//! it -- the vault listing the entitlements router shells out for, and the
//! Wisent Identity project the account holder's session is proved against --
//! because the operator's vault and the production identity project are not
//! test fixtures and a test that mutated either would be the incident it is
//! meant to prevent.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde_json::{json, Value};

use crate::authority::{spawn_identity_authority, ACCOUNT_BEARER, ORGANIZATION_ID};
use crate::support::{write_isolated_vault, TestDirectory};

/// The console identity: `brama-desktop` with no model allowlist is what
/// `require_brama_desktop` accepts, and it is the only proof of the
/// deployment scope.
pub const CONSOLE_BEARER: &str = "brama-pool-console-bearer";
/// A model-scoped workload bearer, the shape the workload authority always
/// resolves, bound to the agent that signs beside it.
pub const AGENT_BEARER: &str = "brama-pool-agent-bearer";
/// A bearer that proves nothing beyond transport.
pub const STRANGER_BEARER: &str = "brama-pool-stranger-bearer";
/// `probierz` is one of Brama's strict central request-signing projections, so
/// its secret is read straight from `BRAMA_REQUEST_SIGN_IDENTITIES` and no
/// capability broker or vault is involved in verifying a signature.
pub const AGENT: &str = "probierz";
pub const AGENT_SIGNING_SECRET: &str = "pool-capability-agent-signing-secret";
/// The other agent in the isolated vault, owned by nobody in this test: it is
/// what proves a scoped answer is narrowed rather than merely filtered.
pub const OTHER_AGENT: &str = "lem";
pub const POOL: &str = "/v1/subscription-pool";

fn available_port() -> u16 {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .expect("reserve loopback port")
        .local_addr()
        .expect("reserved port")
        .port()
}

pub struct Gateway {
    child: Child,
    origin: String,
    client: Client,
    directory: TestDirectory,
}

impl Gateway {
    /// Start the released binary over an isolated vault holding exactly the
    /// accounts this story needs.
    pub fn start(story: &str, vault_items: &[Value]) -> Self {
        let directory = TestDirectory::new(story);
        let root = directory.path().to_owned();
        let router = write_isolated_vault(&root, vault_items);
        let routes = root.join("routes.json");
        std::fs::write(&routes, br#"{"deployments":[],"routes":{},"fallbacks":{}}"#)
            .expect("write the isolated route registry");
        // The gateway refuses a route registry that group or other can read,
        // so the isolated one is protected exactly as a deployment's is.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&routes, std::fs::Permissions::from_mode(0o600))
                .expect("protect the isolated route registry");
        }
        std::fs::create_dir_all(root.join("home")).expect("isolated HOME");

        let identities = json!([
            {"client_id": "brama-desktop", "token": CONSOLE_BEARER},
            {
                "client_id": "brama-pool-agent-client",
                "token": AGENT_BEARER,
                "agent_id": AGENT,
                "allowed_models": ["best"],
            },
            {"client_id": "brama-pool-stranger", "token": STRANGER_BEARER},
        ]);
        let port = available_port();
        let authority = spawn_identity_authority();
        let mut child = Command::new(env!("CARGO_BIN_EXE_brama"))
            .args([
                "serve",
                "--port",
                &port.to_string(),
                "--local-credentials-stdin",
            ])
            .env("HOME", root.join("home"))
            .env(
                "BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES",
                identities.to_string(),
            )
            .env(
                "BRAMA_REQUEST_SIGN_IDENTITIES",
                json!({AGENT: AGENT_SIGNING_SECRET}).to_string(),
            )
            .env(
                "BRAMA_WISENT_AUTH_URL",
                format!("http://127.0.0.1:{authority}"),
            )
            .env("ENTITLEMENTS_ROUTER_BIN", &router)
            .env("BRAMA_INFERENCE_ROUTES_FILE", &routes)
            .env("BRAMA_STATE_DIR", root.join("state"))
            .env("BRAMA_SUBSCRIPTION_USAGE_FILE", root.join("usage.json"))
            .env(
                "BRAMA_DONATED_SUBSCRIPTIONS_FILE",
                root.join("donated.json"),
            )
            .env("BRAMA_PERF_PATH", root.join("perf.json"))
            // No background sweep may read a provider or rotate a grant while
            // a story is asserting what the pool says right now.
            .env("BRAMA_PLAN_USAGE_SWEEP_SECS", "0")
            .env("BRAMA_CREDENTIAL_REFRESH_INTERVAL_SECS", "0")
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
            .timeout(Duration::from_secs(30))
            .build()
            .expect("HTTP client");
        let deadline = Instant::now() + Duration::from_secs(20);
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
                    directory,
                };
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
        let mut refused = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut refused);
        }
        panic!("the real Brama binary did not bind: {refused}");
    }

    pub fn state_dir(&self) -> PathBuf {
        self.directory.path().join("state")
    }

    pub fn donated_file(&self) -> PathBuf {
        self.directory.path().join("donated.json")
    }

    /// One request with exactly the headers the named audience presents.
    pub fn request(
        &self,
        method: reqwest::Method,
        bearer: Option<&str>,
        body: Option<&Value>,
        signed_as: Option<(&str, &str)>,
        organization: bool,
    ) -> (u16, Value) {
        let raw = body.map(|body| serde_json::to_vec(body).expect("request body"));
        let mut request = self
            .client
            .request(method, format!("{}{POOL}", self.origin));
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
        }
        if organization {
            request = request.header("X-Wisent-Organization-ID", ORGANIZATION_ID);
        }
        if let Some((agent, secret)) = signed_as {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_secs() as i64;
            let signature = brama::crypto::hmac_auth::compute_signature(
                agent,
                timestamp,
                raw.as_deref().unwrap_or_default(),
                secret.as_bytes(),
            )
            .expect("sign the request as the agent would");
            request = request
                .header("x-agent-id", agent)
                .header("x-agent-timestamp", timestamp.to_string())
                .header("x-agent-signature", signature);
        }
        if let Some(raw) = raw {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(raw);
        }
        let response = request.send().expect("Brama response");
        let status = response.status().as_u16();
        let body = response.json().expect("Brama JSON response");
        (status, body)
    }

    pub fn console(&self, method: reqwest::Method, body: Option<&Value>) -> (u16, Value) {
        self.request(method, Some(CONSOLE_BEARER), body, None, false)
    }

    pub fn account(&self, method: reqwest::Method, body: Option<&Value>) -> (u16, Value) {
        self.request(method, Some(ACCOUNT_BEARER), body, None, true)
    }

    pub fn agent(&self, method: reqwest::Method, body: Option<&Value>) -> (u16, Value) {
        self.request(
            method,
            Some(AGENT_BEARER),
            body,
            Some((AGENT, AGENT_SIGNING_SECRET)),
            false,
        )
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Every subscription id the pool answered, in order.
pub fn answered_ids(report: &Value) -> Vec<String> {
    report["subscriptions"]
        .as_array()
        .expect("the pool answers a subscriptions array")
        .iter()
        .map(|row| {
            row["id"]
                .as_str()
                .expect("every pool row names its subscription")
                .to_owned()
        })
        .collect()
}

/// The refusal envelope, field by field, as a caller reads it.
pub fn refusal(body: &Value) -> (String, String, String) {
    (
        body["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        body["error"]["type"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    )
}

/// The pool answered from its declaration, and told this caller nothing about
/// an account it was not answered about.
///
/// A complete answer is not the same as `ok`. An account whose plan usage has
/// never been read makes the report incomplete and says so per account, which
/// is the contract: a missing measurement is never reported as zero usage. A
/// failed inventory or an unreadable ledger is a different matter -- it means
/// the declaration behind the answer was not read at all -- and so is a
/// refusal naming an account outside the narrowing, which would leak the
/// existence of somebody else's account.
pub fn assert_pool_answered(report: &Value) {
    let answered = answered_ids(report);
    for error in report["errors"]
        .as_array()
        .expect("the pool answers an errors array")
    {
        let point = error["failure_point"].as_str().unwrap_or_default();
        assert!(
            point != "brama.subscriptions.discovery" && point != "brama.subscriptions.ledger",
            "the pool could not read its own declaration: {error}"
        );
        if let Some(subscription) = error
            .pointer("/context/subscription")
            .and_then(Value::as_str)
        {
            assert!(
                answered.iter().any(|id| id == subscription),
                "the pool named {subscription} to a caller it did not answer about: {report}"
            );
        }
    }
}
