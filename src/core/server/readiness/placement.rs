//! Whether this process is the gateway the registry places, asked once.
//!
//! Brama runs on one host per fleet. Everything else is a window onto it: the
//! Desktop app reads it, clients are routed to it, and its sweep is the one
//! that signs accounts in. A second gateway on a laptop is not a spare -- it
//! reads the same vault, drives the same Weles, and answers questions nobody
//! is asking it, while the operator reading it believes he is looking at the
//! host that serves traffic.
//!
//! Stado owns placement, so this asks Stado rather than guessing from a
//! hostname or a config file: `service directory connect brama` answers which
//! host the service is placed on and which machine is asking. The answer is
//! cached for the process, because placement changes with a registry edit and
//! a restart, not between two readiness reads.

use std::sync::LazyLock;
use std::time::Duration;

use serde_json::{json, Value};

use crate::subscription_dispatch::sign_in::Blocked;

/// How long Stado may take to answer. Placement is one small read; a slow
/// answer is reported as unknown rather than holding a readiness reply open.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

/// What the registry says about where this gateway belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Placement {
    /// The host the registry places brama on, when Stado could say.
    pub(super) placed_on: Option<String>,
    /// The machine this process is running on, as Stado names it.
    pub(super) this_host: Option<String>,
    /// Why the answer is not a comparison, when it is not one.
    pub(super) detail: Option<String>,
}

impl Placement {
    /// Whether this process is the gateway clients are routed to. Unknown
    /// placement is not reported as drift: a registry this process cannot read
    /// is its own fault to state, and calling every unreadable answer a second
    /// gateway would cry wolf on every laptop that runs the test suite.
    pub(super) fn is_placed_host(&self) -> bool {
        match (self.placed_on.as_deref(), self.this_host.as_deref()) {
            (Some(placed_on), Some(this_host)) => placed_on == this_host,
            _ => true,
        }
    }

    /// The drift reason, when this gateway is not the placed one.
    pub(super) fn blocked(&self) -> Option<Blocked> {
        let placed_on = self.placed_on.clone()?;
        let this_host = self.this_host.clone()?;
        (placed_on != this_host).then_some(Blocked::NotPlacedHost {
            placed_on,
            this_host,
        })
    }

    /// The readiness object.
    pub(super) fn to_json(&self) -> Value {
        json!({
            "placed_on": self.placed_on,
            "this_host": self.this_host,
            "is_placed_host": self.is_placed_host(),
            "detail": self
                .blocked()
                .map(|blocked| blocked.detail())
                .or_else(|| self.detail.clone()),
        })
    }
}

/// The Stado binary that answers placement, the same resolution the sign-in
/// path uses to find Weles.
fn stado_binary() -> std::path::PathBuf {
    std::env::var("BRAMA_STADO_BIN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            std::path::PathBuf::from(home)
                .join(".stado")
                .join("bin")
                .join("stado")
        })
}

async fn read_placement() -> Placement {
    let stado = stado_binary();
    let output = tokio::time::timeout(
        LOOKUP_TIMEOUT,
        tokio::process::Command::new(&stado)
            .kill_on_drop(true)
            .args([
                "service",
                "directory",
                "connect",
                "brama",
                "--no-verify",
                "--json",
            ])
            .output(),
    )
    .await;
    let output = match output {
        Ok(Ok(output)) if output.status.success() => output,
        Ok(Ok(output)) => {
            let said: String = String::from_utf8_lossy(&output.stderr)
                .trim()
                .chars()
                .take(300)
                .collect();
            return Placement {
                placed_on: None,
                this_host: None,
                detail: Some(format!(
                    "Stado could not say where brama is placed, so whether this process is the \
                     gateway clients reach is unknown: {said}"
                )),
            };
        }
        Ok(Err(error)) => {
            return Placement {
                placed_on: None,
                this_host: None,
                detail: Some(format!(
                    "the Stado CLI at {} could not be run to read brama's placement: {error}",
                    stado.display()
                )),
            }
        }
        Err(_) => {
            return Placement {
                placed_on: None,
                this_host: None,
                detail: Some(format!(
                    "reading brama's placement through {} did not answer within {} seconds",
                    stado.display(),
                    LOOKUP_TIMEOUT.as_secs()
                )),
            }
        }
    };
    let document: Value = match serde_json::from_slice(&output.stdout) {
        Ok(document) => document,
        Err(error) => {
            return Placement {
                placed_on: None,
                this_host: None,
                detail: Some(format!("Stado's placement answer is not JSON: {error}")),
            }
        }
    };
    let field = |name: &str| {
        document
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|value| !value.trim().is_empty())
    };
    Placement {
        placed_on: field("placed_on"),
        this_host: field("from").or_else(|| field("checked_from")),
        detail: None,
    }
}

/// The placement of this gateway, read once per process.
pub(super) async fn placement() -> &'static Placement {
    static PLACEMENT: LazyLock<tokio::sync::OnceCell<Placement>> =
        LazyLock::new(tokio::sync::OnceCell::new);
    PLACEMENT.get_or_init(read_placement).await
}
