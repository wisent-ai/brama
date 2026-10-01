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

pub(crate) mod pins;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use clap::Subcommand;
use serde_json::Value;

#[derive(Subcommand)]
pub(crate) enum WorkloadCommand {
    /// Grant each agent of this installation's workload exactly the vault
    /// coordinates its capability routes name, bound to its public key
    Register,
    /// Withdraw what `register` granted: revoke the vault grant of every
    /// agent this installation's workload names
    Deregister,
    /// Succeed when REGISTRY pins exactly this process's uid, gid and
    /// BINARY's resolved path and SHA-256; otherwise name the first mismatch
    Check {
        registry: PathBuf,
        #[arg(long)]
        binary: PathBuf,
    },
}

fn check(registry: &Path, binary: &Path) -> Result<(), String> {
    match pins::mismatches(registry, binary)?.into_iter().next() {
        Some(mismatch) => Err(format!("workload registry disagrees on {mismatch}")),
        None => Ok(()),
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

pub(crate) fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default())
}

/// `~/.config/brama/service.env` as `NAME=value` settings, quotes removed.
pub(crate) fn service_settings(home: &Path) -> Result<BTreeMap<String, String>, String> {
    let path = home.join(".config/brama/service.env");
    let text =
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(name, _)| !name.trim_start().starts_with('#'))
        .map(|(name, value)| {
            (
                name.trim().to_string(),
                value.trim().trim_matches(['\'', '"']).to_string(),
            )
        })
        .collect())
}

fn public_key_pem(raw: &str) -> Result<String, String> {
    let key = STANDARD
        .decode(raw)
        .map_err(|error| format!("proof key is not base64: {error}"))?;
    let mut der = ED25519_SPKI_PREFIX.to_vec();
    der.extend(key);
    let body = STANDARD.encode(der);
    let lines: Vec<&str> = body
        .as_bytes()
        .chunks(PEM_LINE)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or(""))
        .collect();
    Ok(format!(
        "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
        lines.join("\n")
    ))
}

/// What `register` and `deregister` both act through: the entitlements
/// router, the vault, the workload registry, and the service settings the
/// router runs with.
struct Installation {
    home: PathBuf,
    settings: BTreeMap<String, String>,
    router: String,
    vault: String,
    registry_path: PathBuf,
    workload: Value,
}

fn installation() -> Result<Installation, String> {
    let home = home();
    let settings = service_settings(&home)?;
    let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let setting = |name: &str| {
        settings
            .get(name)
            .cloned()
            .filter(|value| !value.is_empty())
    };
    // The launcher passes these directly, and then they are authoritative: it
    // has just provisioned the directory whose key must be registered. Outside
    // the launcher, `current` is authoritative for executable and trust
    // material; service.env can keep paths from an older digest.
    let running =
        std::fs::canonicalize(home.join(".stado/services/brama/current")).unwrap_or_default();
    let root = if running.join("darwin-arm").is_dir() {
        running.join("darwin-arm")
    } else {
        running
    };
    let current_router = root.join("bin/skarbiec-entitlements-router");
    let current_config = root.join("etc/brama-skarbiec");
    let router = env("ENTITLEMENTS_ROUTER_BIN")
        .or_else(|| {
            current_router
                .is_file()
                .then(|| current_router.display().to_string())
        })
        .or_else(|| setting("ENTITLEMENTS_ROUTER_BIN"));
    let vault = env("SKARBIEC_VAULT_FILE").or_else(|| setting("SKARBIEC_VAULT_FILE"));
    let config_dir = env("BRAMA_SKARBIEC_CONFIG_DIR")
        .or_else(|| {
            current_config
                .is_dir()
                .then(|| current_config.display().to_string())
        })
        .or_else(|| setting("BRAMA_SKARBIEC_CONFIG_DIR"));
    let (Some(router), Some(vault)) = (router, vault) else {
        return Err("ENTITLEMENTS_ROUTER_BIN and SKARBIEC_VAULT_FILE must be named by the running installation, environment, or service env".into());
    };
    let registry_path = PathBuf::from(config_dir.unwrap_or_default()).join("registry.json");
    let registry: Value = std::fs::read(&registry_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| format!("no workload registry at {}", registry_path.display()))?;
    let workload = registry
        .get("workloads")
        .and_then(Value::as_object)
        .and_then(|workloads| workloads.values().next())
        .cloned()
        .unwrap_or(Value::Null);
    Ok(Installation { home, settings, router, vault, registry_path, workload })
}

fn agent_ids(workload: &Value) -> Vec<&str> {
    workload
        .get("agent_ids")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// The router's own words for a refusal: stderr, or stdout when stderr is empty.
fn refusal_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = if stderr.trim().is_empty() { String::from_utf8_lossy(&output.stdout).trim().to_string() } else { stderr.trim().to_string() };
    detail.replace('\n', " ")
}

/// The inverse of `register`. Revoking a grant the vault does not hold is
/// not an error at the router, so a rerun is harmless; any refusal stops the
/// run with the agent and the router's answer named.
fn deregister() -> Result<(), String> {
    let Installation { settings, router, registry_path, workload, .. } = installation()?;
    let agents = agent_ids(&workload);
    if agents.is_empty() {
        return Err(format!("{} names no agent whose grant could be revoked", registry_path.display()));
    }
    println!("registry: {}", registry_path.display());
    for agent in agents {
        let revoked = Command::new(&router)
            .args(["grant", "revoke", agent])
            .envs(&settings)
            .output()
            .map_err(|error| format!("grant revoke for {agent} could not run: {error}"))?;
        if !revoked.status.success() {
            return Err(format!("grant revoke refused {agent}: {}", refusal_detail(&revoked)));
        }
        println!("{agent}: revoked");
    }
    Ok(())
}

fn register() -> Result<(), String> {
    let Installation { home, settings, router, vault, registry_path, workload } = installation()?;
    let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let setting = |name: &str| {
        settings
            .get(name)
            .cloned()
            .filter(|value| !value.is_empty())
    };
    let public_key = workload
        .get("proof_key")
        .and_then(Value::as_str)
        .unwrap_or("");
    let agents = agent_ids(&workload);
    if public_key.is_empty() || agents.is_empty() {
        return Err(format!(
            "{} names no proof key or agent to bind it to",
            registry_path.display()
        ));
    }
    // On a running host the routes table is not beside the vault the broker
    // opens; looking only there grants nothing and leaves every redemption
    // denied, silently.
    let mut candidates: Vec<PathBuf> = env("SKARBIEC_CAPABILITY_ROUTES_FILE")
        .or_else(|| setting("SKARBIEC_CAPABILITY_ROUTES_FILE"))
        .map(PathBuf::from)
        .into_iter()
        .collect();
    candidates.push(Path::new(&vault).with_file_name("capability-routes.json"));
    candidates.push(home.join(".stado/capability-routes.json"));
    let Some(routes_path) = candidates.iter().find(|path| path.is_file()) else {
        let named: Vec<String> = candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        return Err(format!(
            "no capability routes table found at {}",
            named.join(", ")
        ));
    };
    let routes: Value = std::fs::read(routes_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| format!("{} is not JSON", routes_path.display()))?;
    let capabilities: BTreeSet<String> = routes
        .as_object()
        .into_iter()
        .flat_map(|entries| entries.values())
        .filter_map(|entry| {
            let item = entry
                .get("item")?
                .as_str()
                .filter(|value| !value.is_empty())?;
            let field = entry
                .get("field")?
                .as_str()
                .filter(|value| !value.is_empty())?;
            Some(format!("acquire:{item}#{field}"))
        })
        .collect();
    println!("routes:   {}", routes_path.display());
    if capabilities.is_empty() {
        return Err(format!(
            "{} maps nothing, so there is nothing to grant",
            routes_path.display()
        ));
    }
    let capabilities: Vec<String> = capabilities.into_iter().collect();
    println!("registry: {}", registry_path.display());
    println!("agents:   {}", agents.join(", "));
    println!("granting: {}", capabilities.join(", "));
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
                .envs(&settings)
                .output()
        });
        let _ = std::fs::remove_file(&key_file);
        let minted =
            minted.map_err(|error| format!("grant issue for {agent} could not run: {error}"))?;
        let stdout = String::from_utf8_lossy(&minted.stdout);
        if !minted.status.success() {
            return Err(format!("grant issue refused {agent}: {}", refusal_detail(&minted)));
        }
        let answer: Value = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
        println!(
            "{agent}: workload_bound={} expires_at={}",
            answer.get("workload_bound").unwrap_or(&Value::Null),
            answer.get("expires_at").unwrap_or(&Value::Null)
        );
    }
    Ok(())
}

pub(crate) fn run(command: WorkloadCommand) {
    match command {
        WorkloadCommand::Register => {
            if let Err(detail) = register() {
                eprintln!("{detail}");
                std::process::exit(1);
            }
        }
        WorkloadCommand::Deregister => {
            if let Err(detail) = deregister() {
                eprintln!("{detail}");
                std::process::exit(1);
            }
        }
        WorkloadCommand::Check { registry, binary } => {
            if let Err(detail) = check(&registry, &binary) {
                eprintln!("{detail}");
                std::process::exit(1);
            }
        }
    }
}
