//! `brama workload register`: register this installation's workload key with
//! the vault that guards it.
//!
//! The broker will not redeem a capability unless the vault holds a live
//! token entry for the agent the capability was issued to, carrying that
//! workload's public key. `provision-skarbiec-trust` generates the key pair
//! and records the public half in `registry.json`; nothing then told the
//! vault about it, so every redemption was denied with `capability
//! redemption denied` — a message that names neither the agent nor the
//! missing entry.
//!
//! This closes that gap from what the installation already holds: the public
//! key comes out of the installation's own registry, and the capabilities out
//! of `capability-routes.json`, so the token grants exactly the vault
//! coordinates the broker will be asked to read and nothing more. The
//! private half never leaves the installation, and nothing here prints a
//! secret. The launcher runs it on every start.
//!
//! `brama workload deregister` is its inverse: it revokes the vault grant of
//! every agent the same registry names, through the same router, and exits
//! non-zero with the router's answer on the first refusal.

mod grant;
mod installation;
pub(crate) mod pins;
mod routes;

use installation::{agent_ids, installation, refusal_detail, Installation};
pub(crate) use installation::{home, service_settings};

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Subcommand;
use serde_json::{json, Value};

#[derive(Subcommand)]
pub(crate) enum WorkloadCommand {
    /// Grant each agent of this installation's workload exactly the vault
    /// coordinates its capability routes name, bound to its public key
    Register {
        /// Print the grants as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Withdraw what `register` granted: revoke the vault grant of every
    /// agent this installation's workload names
    Deregister {
        /// Print the revocations as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Succeed when REGISTRY pins exactly this process's uid, gid and
    /// BINARY's resolved path and SHA-256; otherwise name the first mismatch
    Check {
        registry: PathBuf,
        #[arg(long)]
        binary: PathBuf,
        /// Print the agreement as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

fn check(registry: &Path, binary: &Path) -> Result<Value, String> {
    match pins::mismatches(registry, binary)?.into_iter().next() {
        Some(mismatch) => Err(format!("workload registry disagrees on {mismatch}")),
        None => Ok(json!({
            "registry": registry.display().to_string(),
            "binary": binary.display().to_string(),
            "pinned": true,
        })),
    }
}

/// The inverse of `register`. Revoking a grant the vault does not hold is
/// not an error at the router, so a rerun is harmless; any refusal stops the
/// run with the agent and the router's answer named.
fn deregister() -> Result<Value, String> {
    let Installation {
        settings,
        router,
        registry_path,
        workload,
        ..
    } = installation()?;
    let agents = agent_ids(&workload)?;
    if agents.is_empty() {
        return Err(format!(
            "{} names no agent whose grant could be revoked",
            registry_path.display()
        ));
    }
    let mut revoked_agents = Vec::with_capacity(agents.len());
    for agent in agents {
        let revoked = stado_wait::output(
            Command::new(&router)
                .args(["grant", "revoke", agent])
                .envs(&settings),
        )
        .map_err(|error| format!("grant revoke for {agent} could not run: {error}"))?;
        if !revoked.status.success() {
            return Err(format!(
                "grant revoke refused {agent}: {}",
                refusal_detail(&revoked)
            ));
        }
        revoked_agents.push(agent);
    }
    Ok(json!({ "registry": registry_path.display().to_string(), "revoked": revoked_agents }))
}

pub(crate) fn run(command: WorkloadCommand) {
    let (answer, json) = match command {
        WorkloadCommand::Register { json } => (grant::register(), json),
        WorkloadCommand::Deregister { json } => (deregister(), json),
        WorkloadCommand::Check {
            registry,
            binary,
            json,
        } => (check(&registry, &binary), json),
    };
    match answer {
        Ok(answer) => {
            super::print_answer(&answer, json);
            // A register that re-granted some agents and was refused for
            // others is printed whole, then ends as the failure it is.
            if let Some(refused) = answer
                .get("refused")
                .and_then(Value::as_array)
                .filter(|refused| !refused.is_empty())
            {
                let named: Vec<String> = refused
                    .iter()
                    .map(|item| format!("{}: {}", item["agent"], item["cause"]))
                    .collect();
                crate::cli::refused(format!(
                    "brama workload register: {} agent(s) were not re-granted: {}",
                    named.len(),
                    named.join("; ")
                ));
            }
        }
        Err(detail) => crate::cli::refused(detail),
    }
}
