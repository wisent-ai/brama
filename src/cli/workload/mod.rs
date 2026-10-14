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

mod installation;
pub(crate) mod pins;
mod routes;

use installation::{agent_ids, installation, refusal_detail, Installation};
pub(crate) use installation::{home, service_settings};

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::{engine::general_purpose::STANDARD, Engine as _};
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

/// The fixed SubjectPublicKeyInfo header of an Ed25519 key: the registry
/// records the raw key and `grant issue` validates an SPKI PEM, so this
/// prefix makes the two the same key.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];
/// Line width of a PEM body.
const PEM_LINE: usize = 76;
/// Owner read and write only.
const OWNER_ONLY: u32 = 0o600;

fn public_key_pem(raw: &str) -> Result<String, String> {
    let key = STANDARD
        .decode(raw)
        .map_err(|error| format!("proof key is not base64: {error}"))?;
    let mut der = ED25519_SPKI_PREFIX.to_vec();
    der.extend(key);
    let body = STANDARD.encode(der);
    let lines = body
        .as_bytes()
        .chunks(PEM_LINE)
        .map(|chunk| {
            std::str::from_utf8(chunk)
                .map_err(|error| format!("PEM body line is not text: {error}"))
        })
        .collect::<Result<Vec<&str>, String>>()?;
    Ok(format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
        lines.join("\n")
    ))
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
    let agents = agent_ids(&workload);
    if agents.is_empty() {
        return Err(format!(
            "{} names no agent whose grant could be revoked",
            registry_path.display()
        ));
    }
    let mut revoked_agents = Vec::with_capacity(agents.len());
    for agent in agents {
        let revoked = Command::new(&router)
            .args(["grant", "revoke", agent])
            .envs(&settings)
            .output()
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

/// The runtime grant's stated lifetime: the largest integer JSON carries
/// exactly (`Number.MAX_SAFE_INTEGER`), the value Brama's Skarbiec policy
/// writes for "no lifetime chosen", because Skarbiec requires an acquire grant
/// to state one.
const RUNTIME_GRANT_TTL_SECONDS: u64 = (1 << 53) - 1;

fn register() -> Result<Value, String> {
    let Installation {
        home,
        settings,
        router,
        vault,
        registry_path,
        workload,
    } = installation()?;
    let public_key = workload
        .get("proof_key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty());
    let agents = agent_ids(&workload);
    let (Some(public_key), false) = (public_key, agents.is_empty()) else {
        return Err(format!(
            "{} names no proof key or agent to bind it to",
            registry_path.display()
        ));
    };
    let (routes_path, capabilities) = routes::capabilities(&home, &vault, &settings)?;
    let mut granted = Vec::with_capacity(agents.len());
    let pem = public_key_pem(public_key)?;
    let key_file =
        registry_path.with_file_name(format!(".workload-key-{}.pem", std::process::id()));
    for agent in agents {
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(OWNER_ONLY)
            .open(&key_file)
            .and_then(|mut file| file.write_all(pem.as_bytes()));
        let minted = written.and_then(|()| {
            Command::new(&router)
                .args([
                    "grant",
                    "issue",
                    agent,
                    "--capabilities",
                    &capabilities.join(","),
                ])
                .arg("--workload-public-key-file")
                .arg(&key_file)
                .arg("--replace-capabilities")
                // Skarbiec 0.4.15 refuses a grant that does not state its
                // lifetime, and an acquire grant may not be `--until-revoked`
                // ("an acquire grant lends its lifetime to every bearer it
                // mints; state --ttl-seconds"): 0.4.53 failed every boot on
                // the first refusal, 0.4.54 on the second. Brama chooses no
                // lifetime for its runtime grant; its Skarbiec policy
                // (libexec/generate-skarbiec-config.mjs) writes the largest
                // integer JSON carries exactly, which reads as none, and the
                // grant states the same.
                .arg("--ttl-seconds")
                .arg(RUNTIME_GRANT_TTL_SECONDS.to_string())
                .envs(&settings)
                .output()
        });
        // The next agent writes the same file with `create_new`; one left
        // behind would refuse it as existing instead of by this cause.
        if let Some(left) = brama::core::staging::remove(&key_file) {
            return Err(format!(
                "the workload public key file for {agent} could not be removed after grant issue: {left}"
            ));
        }
        let minted =
            minted.map_err(|error| format!("grant issue for {agent} could not run: {error}"))?;
        let stdout = String::from_utf8_lossy(&minted.stdout);
        if !minted.status.success() {
            return Err(format!(
                "grant issue refused {agent}: {}",
                refusal_detail(&minted)
            ));
        }
        let answer: Value = serde_json::from_str(stdout.trim()).map_err(|error| {
            format!("grant issue for {agent} succeeded but answered no JSON: {error}")
        })?;
        // The grant was issued for this workload's key; an answer that does
        // not say it is bound to it, or does not state its expiry, is not
        // recorded as a grant with blanks.
        if answer.get("workload_bound") != Some(&Value::Bool(true)) {
            return Err(format!(
                "grant issue for {agent} did not answer workload_bound=true for the key it was given: {answer}"
            ));
        }
        let Some(expires_at) = answer.get("expires_at") else {
            return Err(format!(
                "grant issue for {agent} answered no expires_at: {answer}"
            ));
        };
        granted.push(json!({
            "agent": agent,
            "workload_bound": true,
            "expires_at": expires_at,
        }));
    }
    Ok(json!({
        "routes": routes_path.display().to_string(),
        "registry": registry_path.display().to_string(),
        "granting": capabilities,
        "granted": granted,
    }))
}

pub(crate) fn run(command: WorkloadCommand) {
    let (answer, json) = match command {
        WorkloadCommand::Register { json } => (register(), json),
        WorkloadCommand::Deregister { json } => (deregister(), json),
        WorkloadCommand::Check {
            registry,
            binary,
            json,
        } => (check(&registry, &binary), json),
    };
    match answer {
        Ok(answer) => super::print_answer(&answer, json),
        Err(detail) => {
            eprintln!("{detail}");
            std::process::exit(1);
        }
    }
}
