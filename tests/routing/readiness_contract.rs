//! `/readyz` answers as soon as the direct-provider half of the sweep is
//! known, and says which half it is answering from.
//!
//! On 2026-09-17 the 0.4.24 candidate on charless-mac-mini stayed at
//! `readiness check has not completed` (503) past Stado's 90-second window
//! because the subscription half of its first sweep was still running, and
//! the release was quarantined by the gateway it replaces. The direct
//! provider half costs one broker round trip; a gateway that obtained one
//! credential there carries traffic and must say so at once.
//!
//! The real binary serves real HTTP on loopback in its standalone shape,
//! with a local provider credential on stdin in place of a vault; the
//! credential is never sent anywhere, because readiness obtains it and does
//! not spend it.

use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use serde_json::{json, Value};

const CONSOLE_BEARER: &str = "brama-readiness-console-bearer";

/// How long a serving verdict may take after the process binds: the
/// direct-provider half of one sweep, with room for a slow machine, and far
/// under the 90 seconds a rollout allows.
const READY_WITHIN: Duration = Duration::from_secs(10);

struct Gateway {
    child: Child,
    origin: String,
    client: Client,
    scratch: std::path::PathBuf,
}

impl Gateway {
    fn start(local_credentials: &str) -> Self {
        let scratch = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/target"))
            .join(format!(
                "readiness-contract-{}-{}",
                std::process::id(),
                local_credentials.len()
            ));
        std::fs::create_dir_all(&scratch).expect("scratch under the ignored build directory");
        let routes = scratch.join("routes.json");
        std::fs::write(&routes, b"{}").expect("write the isolated route registry");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&routes, std::fs::Permissions::from_mode(0o600))
                .expect("protect the isolated route registry");
        }
        let port = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("reserve loopback port")
            .local_addr()
            .expect("reserved port")
            .port();
        let mut child = Command::new(env!("CARGO_BIN_EXE_brama"))
            .args([
                "serve",
                "--port",
                &port.to_string(),
                "--local-credentials-stdin",
            ])
            .env(
                "BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES",
                json!([{"client_id": "brama-desktop", "token": CONSOLE_BEARER}]).to_string(),
            )
            .env("BRAMA_INFERENCE_ROUTES_FILE", &routes)
            .env("BRAMA_STATE_DIR", scratch.join("state"))
            .env("BRAMA_PERF_PATH", scratch.join("perf.json"))
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
            .write_all(local_credentials.as_bytes())
            .expect("hand the standalone credential store over");
        let origin = format!("http://127.0.0.1:{port}");
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("HTTP client");
        let bound = Instant::now() + Duration::from_secs(15);
        while Instant::now() < bound {
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

    fn readyz(&self) -> (u16, Value) {
        let response = self
            .client
            .get(format!("{}/readyz", self.origin))
            .send()
            .expect("readyz answers");
        let status = response.status().as_u16();
        let body = response.json().expect("readyz is JSON");
        (status, body)
    }

    /// The first readiness answer that is no longer the pending placeholder.
    fn first_verdict(&self) -> (u16, Value, Duration) {
        let started = Instant::now();
        loop {
            let (status, body) = self.readyz();
            if body["reason"] != "readiness check has not completed" {
                return (status, body, started.elapsed());
            }
            assert!(
                started.elapsed() < READY_WITHIN,
                "readiness stayed pending past {READY_WITHIN:?}: {body}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
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
fn a_gateway_holding_one_direct_credential_is_ready_before_the_sweep_ends() {
    let gateway = Gateway::start(r#"{"openai":"readiness-contract-key"}"#);
    let (status, body, took) = gateway.first_verdict();
    assert_eq!(status, 200, "a direct credential was obtained: {body}");
    assert_eq!(body["ready"], true, "{body}");
    assert!(
        body["providers"]
            .as_array()
            .is_some_and(|providers| providers.iter().any(|provider| {
                provider["provider"] == "openai" && provider["credential"] == true
            })),
        "the verdict names the credential it obtained: {body}"
    );
    let reason = body["reason"].as_str().unwrap_or_default();
    assert!(
        reason
            == "a direct provider credential was obtained; the subscription sweep is still running"
            || reason.starts_with("every configured provider credential was obtained"),
        "the verdict says which half it answers from ({took:?}): {reason}"
    );
    if reason.starts_with("a direct provider credential was obtained") {
        assert_eq!(
            body["degraded"], true,
            "an interim verdict is degraded: {body}"
        );
    }
}

#[test]
fn a_gateway_holding_nothing_is_not_ready_and_says_so() {
    let gateway = Gateway::start("{}");
    let (status, body, _) = gateway.first_verdict();
    assert_eq!(status, 503, "{body}");
    assert_eq!(body["ready"], false, "{body}");
    let reason = body["reason"].as_str().unwrap_or_default();
    assert!(
        reason
            == "no direct provider credential was obtained; the subscription sweep is still running"
            || reason == "no provider capability or subscription is configured",
        "{reason}"
    );
}
