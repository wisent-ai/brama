//! `brama workload register`: grant each agent of this installation's
//! workload exactly the vault coordinates its capability routes name, bound
//! to its public key, for the lifetime the operator declares.
//!
//! Skarbiec requires an acquire grant to state its lifetime and refuses
//! `--until-revoked` for one, so that such a grant ends. The lifetime is the
//! operator's: `BRAMA_RUNTIME_GRANT_SECONDS` in the installation's
//! `service.env`, and a registration without it is refused by name. Brama
//! chooses none; it once stated the largest integer JSON carries, a grant
//! that never ends in practice under a label saying no lifetime was chosen.

use std::io::Write;
use std::num::NonZeroU64;
use std::os::unix::fs::OpenOptionsExt;
use std::process::Command;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};

use super::installation::{agent_ids, installation, refusal_detail, Installation};
use super::routes;

/// The declaration in `service.env` that states the runtime grant's lifetime.
const RUNTIME_GRANT_ENV: &str = "BRAMA_RUNTIME_GRANT_SECONDS";

/// The fixed SubjectPublicKeyInfo header of an Ed25519 key: the registry
/// records the raw key and `grant issue` validates an SPKI PEM, so this
/// prefix makes the two the same key.
// https://www.rfc-editor.org/rfc/rfc8410#section-10.1
const ED25519_SPKI_PREFIX: &[u8] = &[
    // https://www.rfc-editor.org/rfc/rfc8410#section-10.1
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21,
    0x00, // https://www.rfc-editor.org/rfc/rfc8410#section-10.1
];
/// Line width of a PEM body.
const PEM_LINE: usize = 64; // https://www.rfc-editor.org/rfc/rfc7468#section-2

/// The runtime grant's lifetime in whole seconds, as the operator declared
/// it, or the refusal naming the declaration. Zero is refused with the
/// parse: a grant that ends as it is issued serves nothing.
fn grant_seconds(installation: &Installation) -> Result<NonZeroU64, String> {
    let declared = installation.settings.get(RUNTIME_GRANT_ENV).ok_or_else(|| {
        format!(
            "{} declares no {RUNTIME_GRANT_ENV}: Skarbiec requires the runtime grant to state its lifetime and Brama chooses none, so state the grant period in whole seconds",
            installation.home.join(".config/brama/service.env").display()
        )
    })?;
    declared.trim().parse::<NonZeroU64>().map_err(|error| {
        format!("{RUNTIME_GRANT_ENV} must be a whole number of seconds above zero; it is {declared:?}: {error}")
    })
}

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

/// Re-grant every agent the workload names: each is issued in turn whatever
/// the one before answered, so a refusal never leaves the registry half
/// re-granted with no record of which half. The answer lists every agent
/// granted (with its expiry) and every agent refused (with why); a run with
/// any refusal is printed whole and then ends non-zero (`workload::run`).
pub(super) fn register() -> Result<Value, String> {
    let installation = installation()?;
    let lifetime = grant_seconds(&installation)?.get();
    let Installation {
        home,
        settings,
        router,
        vault,
        registry_path,
        workload,
    } = installation;
    let public_key = workload
        .get("proof_key")
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty());
    let agents = agent_ids(&workload)?;
    let (Some(public_key), false) = (public_key, agents.is_empty()) else {
        return Err(format!(
            "{} names no proof key or agent to bind it to",
            registry_path.display()
        ));
    };
    let (routes_path, capabilities) = routes::capabilities(&home, &vault, &settings)?;
    let pem = public_key_pem(public_key)?;
    // One name per registration: a file a crashed registration left under a
    // name tied to the process id would refuse every later registration of a
    // process given that id (`create_new`) with its existence instead of a
    // real cause, as the grants file's staging once did.
    let key_file = registry_path.with_file_name(format!(
        ".workload-key-{}.pem",
        uuid::Uuid::new_v4().simple()
    ));
    let issuing = Issuing {
        router: &router,
        settings: &settings,
        capabilities: &capabilities.join(","),
        key_file: &key_file,
        pem: &pem,
        lifetime,
    };
    let mut granted = Vec::with_capacity(agents.len());
    let mut refused = Vec::new();
    for agent in agents {
        match issuing.issue(agent) {
            Ok(expires_at) => granted.push(json!({
                "agent": agent,
                "workload_bound": true,
                "expires_at": expires_at,
            })),
            Err(cause) => refused.push(json!({ "agent": agent, "cause": cause })),
        }
    }
    Ok(json!({
        "routes": routes_path.display().to_string(),
        "registry": registry_path.display().to_string(),
        "granting": capabilities,
        "lifetime_seconds": lifetime,
        "granted": granted,
        "refused": refused,
    }))
}

/// What every agent's grant is issued with.
struct Issuing<'a> {
    router: &'a str,
    settings: &'a std::collections::BTreeMap<String, String>,
    capabilities: &'a str,
    key_file: &'a std::path::Path,
    pem: &'a str,
    lifetime: u64,
}

impl Issuing<'_> {
    /// Issue `agent`'s grant for the workload's key; its expiry, or why not.
    fn issue(&self, agent: &str) -> Result<Value, String> {
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(u32::from(libc::S_IRUSR | libc::S_IWUSR))
            .open(self.key_file)
            .and_then(|mut file| file.write_all(self.pem.as_bytes()));
        let minted = written.and_then(|()| {
            stado_wait::output(
                Command::new(self.router)
                    .args(["grant", "issue", agent, "--capabilities", self.capabilities])
                    .arg("--workload-public-key-file")
                    .arg(self.key_file)
                    .arg("--replace-capabilities")
                    .arg("--ttl-seconds")
                    .arg(self.lifetime.to_string())
                    .envs(self.settings),
            )
        });
        // The next agent writes the same file with `create_new`; one left
        // behind would refuse it as existing instead of by this cause.
        if let Some(left) = brama::core::staging::remove(self.key_file) {
            return Err(format!(
                "the workload public key file could not be removed after grant issue: {left}"
            ));
        }
        let minted = minted.map_err(|error| format!("grant issue could not run: {error}"))?;
        if !minted.status.success() {
            return Err(format!(
                "grant issue refused it: {}",
                refusal_detail(&minted)
            ));
        }
        let stdout = String::from_utf8_lossy(&minted.stdout);
        let answer: Value = serde_json::from_str(stdout.trim())
            .map_err(|error| format!("grant issue succeeded but answered no JSON: {error}"))?;
        // The grant was issued for this workload's key; an answer that does
        // not say it is bound to it, or does not state its expiry, is not
        // recorded as a grant with blanks.
        if answer.get("workload_bound") != Some(&Value::Bool(true)) {
            return Err(format!(
                "grant issue did not answer workload_bound=true for the key it was given: {answer}"
            ));
        }
        answer
            .get("expires_at")
            .cloned()
            .ok_or_else(|| format!("grant issue answered no expires_at: {answer}"))
    }
}
