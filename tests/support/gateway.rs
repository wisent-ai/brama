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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use serde_json::json;

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
    /// Everything the gateway wrote to stderr so far: its log, which is where
    /// its events land. Read by a thread so the child never blocks on a full
    /// pipe.
    log: Arc<Mutex<String>>,
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
        let log = Arc::new(Mutex::new(String::new()));
        let mut stderr = child.stderr.take().expect("the gateway's stderr");
        let captured = Arc::clone(&log);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(read) = stderr.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                if let Ok(mut buffer) = captured.lock() {
                    buffer.push_str(&String::from_utf8_lossy(&chunk[..read]));
                }
            }
        });

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
                    log,
                };
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        let _ = child.wait();
        let refused = log.lock().map(|buffer| buffer.clone()).unwrap_or_default();
        panic!("the real Brama binary did not bind: {refused}");
    }

    /// This gateway's own state area, where its writes land.
    pub fn root(&self) -> &Path {
        self.directory.path()
    }

    /// The gateway's log so far: every event it wrote to stderr, which is the
    /// record an operator reads to attribute a slow or refused request.
    pub fn log(&self) -> String {
        self.log
            .lock()
            .map(|buffer| buffer.clone())
            .unwrap_or_default()
    }

    /// The router is invoked on every call, so an account can stop existing
    /// mid-story exactly as it does when its item is deleted.
    pub fn vault(&self) -> &SkarbiecVault {
        &self.vault
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[path = "pool_report.rs"]
mod pool_report;
// Every story compiles this fixture on its own, and not every story reads the pool.
#[allow(unused_imports)]
pub use pool_report::{answered_ids, assert_pool_answered, refusal};

#[path = "gateway_requests.rs"]
mod requests;
