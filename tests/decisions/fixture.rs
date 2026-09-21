//! What every decision story shares: a real gateway, a registry of its own,
//! and the questions they all ask.
//!
//! The gateway process, the vault, the capability environment and the
//! provider are the real ones; the caller's bearer, the route registry and
//! the state directory are minted per story, so nothing here touches the
//! operator's own registry. Scratch state lives in this package's own build
//! directory and is removed with the story that made it.

#![allow(dead_code)]

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub const BEARER: &str = "brama-decision-real-test-token";
pub const DECISION_ALIAS: &str = "decision-model";
pub const BEST_DECISION_ALIAS: &str = "best-decision-model";
pub const STATE: &str = "Help! My payouts have been failing for 3 days.";
/// The question whose answer must be one of the options it declares.
pub const CHOICE_QUESTION: &str = "department";
/// The question graded against an ordered rubric.
pub const SCORE_QUESTION: &str = "severity";
/// The yes/no question.
pub const NOUL_QUESTION: &str = "is_urgent";
/// The one document version the registry reader accepts.
pub const REGISTRY_SCHEMA_VERSION: u32 = 1;
/// The route the served stories pay for. `openai` has no credits on this
/// account, so this is the OpenRouter route the HTTP API stories already use;
/// a deployment whose capabilities differ names its own.
const DEFAULT_ROUTE: &str = "openrouter/openai/gpt-4o-mini";

pub fn decision_route() -> String {
    std::env::var("BRAMA_DECISION_TEST_ROUTE").unwrap_or_else(|_| DEFAULT_ROUTE.to_string())
}

/// The questions every story asks, so a failure names the answer that broke
/// rather than a prompt that differed.
pub fn questions() -> Value {
    json!({
        NOUL_QUESTION: {
            "type": "noul",
            "instructions": "Does this convey urgency?"
        },
        CHOICE_QUESTION: {
            "type": "choice",
            "instructions": "Which team should handle this?",
            "criteria": {
                "billing": "Payments, invoicing, refunds",
                "technical": "Bugs, outages, integrations",
                "sales": "Pricing, upgrades, new accounts"
            }
        },
        SCORE_QUESTION: {
            "type": "score",
            "instructions": "How severe is this for the customer?",
            "criteria": ["cosmetic", "annoying", "blocking", "outage"]
        }
    })
}

/// The options the choice question declares, read from the question itself so
/// the assertion cannot drift from what was asked.
pub fn declared_options() -> Vec<String> {
    questions()[CHOICE_QUESTION]["criteria"]
        .as_object()
        .expect("the choice question declares options")
        .keys()
        .cloned()
        .collect()
}

/// How many levels the score question grades over, read the same way.
pub fn declared_levels() -> usize {
    questions()[SCORE_QUESTION]["criteria"]
        .as_array()
        .expect("the score question declares levels")
        .len()
}

fn available_port() -> u16 {
    TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .expect("reserve loopback port")
        .local_addr()
        .expect("reserved port")
        .port()
}

/// A private scratch directory inside this package's own build area.
pub fn scratch(story: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("decisions")
        .join(format!("{story}-{}-{nonce}", std::process::id()));
    std::fs::create_dir_all(&path).expect("create test scratch directory");
    path
}

/// An owner-only route registry holding exactly the aliases one story needs.
pub fn registry(root: &Path, routes: &Value) -> PathBuf {
    let path = root.join("inference-routes.json");
    let mut document = serde_json::Map::new();
    document.insert("schema_version".into(), json!(REGISTRY_SCHEMA_VERSION));
    document.insert("deployments".into(), json!([]));
    document.insert("routes".into(), routes.clone());
    std::fs::write(
        &path,
        serde_json::to_vec(&Value::Object(document)).expect("serialize the isolated registry"),
    )
    .expect("write the isolated registry");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("protect the isolated registry");
    }
    path
}

/// The real binary, reading one story's registry.
pub fn brama(registry: &Path, arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_brama"))
        .args(arguments)
        .env("BRAMA_INFERENCE_ROUTES_FILE", registry)
        .output()
        .expect("run the real brama binary")
}

pub struct RealGateway {
    child: Child,
    origin: String,
    scratch: PathBuf,
}

impl RealGateway {
    pub fn start(story: &str, routes: Value) -> Self {
        let port = available_port();
        let scratch = scratch(story);
        let registry = registry(&scratch, &routes);
        // Three identities: the caller under test, and the two clients the
        // gateway refuses to start without — it checks that `wisent-backend`
        // and `weles` are each allowed every alias their own names promise.
        let identities = json!([
            {
                "client_id": "decision-real-test",
                "token": BEARER,
                "allowed_models": [DECISION_ALIAS, BEST_DECISION_ALIAS],
            },
            {
                "client_id": "wisent-backend",
                "token": "brama-decision-real-backend-token",
                "allowed_models": [
                    "wisent-backend",
                    "wisent-backend/evaluation",
                    "wisent-backend/embeddings",
                    "wisent-backend/moderation",
                ],
            },
            {
                "client_id": "weles",
                "token": "brama-decision-real-weles-token",
                "allowed_models": ["weles", "best"],
            },
        ]);
        // A managed gateway refuses to start without the launcher's table, so
        // the story supplies the smallest one that satisfies the contract:
        // the six required aliases, each on a shape its name promises. What
        // the story is about lives in the registry beside it.
        let launcher_aliases = json!({
            "wisent-backend": "best",
            "wisent-backend/evaluation": "best",
            "wisent-backend/embeddings": "openai/embeddings",
            "wisent-backend/moderation": "openai/moderation",
            "weles": "best",
            "best": "best",
        });
        let child = Command::new(env!("CARGO_BIN_EXE_brama"))
            .args(["serve", "--port", &port.to_string()])
            .env(
                "BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES",
                identities.to_string(),
            )
            .env("BRAMA_MODEL_ALIASES", launcher_aliases.to_string())
            .env("BRAMA_INFERENCE_ROUTES_FILE", &registry)
            .env("BRAMA_STATE_DIR", scratch.join("state"))
            .env("BRAMA_PERF_PATH", scratch.join("perf.json"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the real Brama gateway");
        let mut gateway = Self {
            child,
            origin: format!("http://127.0.0.1:{port}"),
            scratch,
        };
        gateway.wait_ready();
        gateway
    }

    fn wait_ready(&mut self) {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("HTTP client");
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().expect("poll gateway") {
                panic!(
                    "the real gateway exited before binding ({status}): {}",
                    self.refusal()
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

    /// What the gateway wrote before it gave up, so a failed start names its
    /// own cause instead of an exit code.
    fn refusal(&mut self) -> String {
        let mut refused = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            use std::io::Read as _;
            let _ = pipe.read_to_string(&mut refused);
        }
        refused
    }

    pub fn post(&self, path: &str, body: &Value) -> (reqwest::StatusCode, Value) {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("HTTP client");
        let response = client
            .post(format!("{}{path}", self.origin))
            .bearer_auth(BEARER)
            .json(body)
            .send()
            .expect("gateway HTTP response");
        let status = response.status();
        (status, response.json().expect("gateway response is JSON"))
    }

    pub fn decide(&self, body: &Value) -> (reqwest::StatusCode, Value) {
        self.post("/v1/decisions", body)
    }
}

impl Drop for RealGateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.scratch);
    }
}
