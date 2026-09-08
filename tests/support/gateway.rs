//! One real gateway over one real Skarbiec vault.
//!
//! Everything here is the product: the released binary serves real HTTP on
//! loopback, resolves bearers through its own identity path, verifies the HMAC
//! trio with its own signing code, shells out to the real entitlements router
//! for its inventory, and writes its own journal and usage ledger. Only the
//! data is isolated -- until this change the inventory was too, behind a
//! three-line `/bin/sh` script that printed a listing in Skarbiec's place.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde_json::{json, Value};

use crate::support::{SkarbiecVault, TestDirectory};

/// `brama-desktop` with no allowlist is what `require_brama_desktop` accepts.
pub const CONSOLE_BEARER: &str = "brama-pool-console-bearer";
/// A model-scoped bearer, the shape every workload identity has.
pub const AGENT_BEARER: &str = "brama-pool-agent-bearer";
pub const STRANGER_BEARER: &str = "brama-pool-stranger-bearer";
/// `probierz` is one of Brama's strict central request-signing projections, so
/// its secret is read straight from `BRAMA_REQUEST_SIGN_IDENTITIES`.
pub const AGENT: &str = "probierz";
pub const AGENT_SIGNING_SECRET: &str = "pool-capability-agent-signing-secret";
/// Owned by nobody here: what proves an answer is narrowed, not filtered.
pub const OTHER_AGENT: &str = "lem";
pub const POOL: &str = "/v1/subscription-pool";
pub const PLAN_USAGE: &str = "/v1/plan-usage";

pub struct Gateway {
    child: Child,
    origin: String,
    client: Client,
    directory: TestDirectory,
    vault: SkarbiecVault,
}

impl Gateway {
    /// Over a real vault holding exactly the accounts this story needs, each
    /// `(owning agent, provider, subscription id)`.
    pub fn start(story: &str, accounts: &[(&str, &str, &str)]) -> Self {
        Self::start_with(story, accounts, &[])
    }

    pub fn start_with(
        story: &str,
        accounts: &[(&str, &str, &str)],
        environment: &[(&str, String)],
    ) -> Self {
        let directory = TestDirectory::new(story);
        let vault = SkarbiecVault::create(story);
        for (agent, provider, subscription) in accounts {
            vault.seed_subscription(agent, provider, subscription);
        }
        let root = directory.path().to_owned();
        // An empty route registry -- every field is `serde(default)` and this
        // deployment declares no local route -- written 0600 because the
        // gateway refuses a registry group or other can read.
        let routes = root.join("routes.json");
        std::fs::write(&routes, b"{}").expect("write the isolated route registry");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&routes, std::fs::Permissions::from_mode(0o600))
                .expect("protect the isolated route registry");
        }

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
        // A port the kernel reserved for this story, never a fixed one.
        let port = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("reserve loopback port")
            .local_addr()
            .expect("reserved port")
            .port();
        let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
        command.args([
            "serve",
            "--port",
            &port.to_string(),
            "--local-credentials-stdin",
        ]);
        // The vault's environment carries HOME, GNUPGHOME and the vault path,
        // and Brama hands its whole environment to the real router child,
        // which is what keeps that child off the operator's vault.
        for (name, value) in vault.environment() {
            command.env(name, value);
        }
        let mut child = command
            .env(
                "BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES",
                identities.to_string(),
            )
            .env(
                "BRAMA_REQUEST_SIGN_IDENTITIES",
                json!({AGENT: AGENT_SIGNING_SECRET}).to_string(),
            )
            .env("ENTITLEMENTS_ROUTER_BIN", vault.router())
            .env("BRAMA_INFERENCE_ROUTES_FILE", &routes)
            .env("BRAMA_STATE_DIR", root.join("state"))
            .env("BRAMA_SUBSCRIPTION_USAGE_FILE", root.join("usage.json"))
            .env(
                "BRAMA_DONATED_SUBSCRIPTIONS_FILE",
                root.join("donated.json"),
            )
            .env("BRAMA_PERF_PATH", root.join("perf.json"))
            // No background sweep may move state under a live assertion.
            .env("BRAMA_PLAN_USAGE_SWEEP_SECS", "0")
            .env("BRAMA_CREDENTIAL_REFRESH_INTERVAL_SECS", "0")
            .envs(
                environment
                    .iter()
                    .map(|(name, value)| (*name, value.as_str())),
            )
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
                    vault,
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

    /// This gateway's own state area, where its writes land.
    pub fn root(&self) -> &Path {
        self.directory.path()
    }

    /// The router is invoked on every call, so an account can stop existing
    /// mid-story exactly as it does when its item is deleted.
    pub fn vault(&self) -> &SkarbiecVault {
        &self.vault
    }

    /// One request, with exactly the headers the named audience presents.
    pub fn request(
        &self,
        path: &str,
        method: reqwest::Method,
        bearer: Option<&str>,
        body: Option<&Value>,
        signed_as: Option<(&str, &str)>,
    ) -> (u16, Value) {
        let raw = body.map(|body| serde_json::to_vec(body).expect("request body"));
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.origin));
        if let Some(bearer) = bearer {
            request = request.bearer_auth(bearer);
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

    pub fn console(
        &self,
        path: &str,
        method: reqwest::Method,
        body: Option<&Value>,
    ) -> (u16, Value) {
        self.request(path, method, Some(CONSOLE_BEARER), body, None)
    }

    pub fn agent(&self, path: &str, method: reqwest::Method, body: Option<&Value>) -> (u16, Value) {
        self.request(
            path,
            method,
            Some(AGENT_BEARER),
            body,
            Some((AGENT, AGENT_SIGNING_SECRET)),
        )
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

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
    let field = |name: &str| body["error"][name].as_str().unwrap_or_default().to_owned();
    (field("code"), field("type"), field("message"))
}

/// The capability answered from its declaration, and told this caller nothing
/// about an account it was not answered about: a failed inventory means the
/// declaration was not read at all, and a refusal naming an account outside
/// the narrowing would leak somebody else's account.
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
