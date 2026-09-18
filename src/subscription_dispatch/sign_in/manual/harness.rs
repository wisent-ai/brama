//! The grants every harness on this machine already holds.
//!
//! Four harnesses sign accounts in on an operator's machine and each keeps
//! the grant somewhere of its own: `omp` in a SQLite store, Claude Code in a
//! credentials file or the macOS Keychain, Codex CLI in `auth.json`, Kimi
//! Code in a credentials file. Brama reads each one the way it was written,
//! never writes any of them, and turns what it finds into the one document
//! its refresh path reads for that provider. On 2026-09-13 the operator
//! asked why only `omp` was read: an account signed into any of them is an
//! account the pool can use.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use zeroize::Zeroizing;

use super::omp;

/// A harness that signs accounts in on this machine and keeps the grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    Omp,
    Claude,
    Codex,
    Kimi,
}

impl Harness {
    pub const ALL: [Harness; 4] = [Harness::Omp, Harness::Claude, Harness::Codex, Harness::Kimi];

    /// The word the operator names it by.
    pub fn name(self) -> &'static str {
        match self {
            Harness::Omp => "omp",
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Kimi => "kimi",
        }
    }

    pub fn parse(name: &str) -> Option<Harness> {
        Harness::ALL
            .into_iter()
            .find(|harness| harness.name() == name.trim())
    }

    /// The providers this harness signs accounts in for.
    pub fn providers(self) -> &'static [&'static str] {
        match self {
            Harness::Omp => &["claude-code", "codex", "kimi"],
            Harness::Claude => &["claude-code"],
            Harness::Codex => &["codex"],
            Harness::Kimi => &["kimi"],
        }
    }

    /// Where this harness keeps its grants, below a home directory.
    pub fn store(self, home: &Path) -> PathBuf {
        match self {
            Harness::Omp => home.join(".omp/agent/agent.db"),
            Harness::Claude => home.join(".claude/.credentials.json"),
            Harness::Codex => home.join(".codex/auth.json"),
            Harness::Kimi => home.join(".kimi-code/credentials/kimi-code.json"),
        }
    }
}

/// One grant a harness holds, already in the shape Brama's refresh path
/// reads for its provider. The document is the secret; everything else is
/// what a list may show.
pub struct HeldGrant {
    pub harness: Harness,
    pub provider: &'static str,
    pub account: Option<String>,
    pub expires_at_ms: Option<i64>,
    pub document: Zeroizing<String>,
}

/// What a list of held grants shows: never the document.
#[derive(Serialize)]
pub struct HeldGrantView {
    pub harness: Harness,
    pub provider: &'static str,
    pub account: Option<String>,
    pub expires_at_ms: Option<i64>,
    pub store: String,
}

/// Every grant every harness under `home` holds for `provider`, or for
/// every provider when none is named. A harness whose store is not there
/// holds nothing; one whose store cannot be read says so.
pub fn held(home: &Path, provider: Option<&str>) -> Result<Vec<HeldGrant>, String> {
    let mut found = Vec::new();
    for harness in Harness::ALL {
        for candidate in harness.providers() {
            if provider.is_some_and(|wanted| wanted != *candidate) {
                continue;
            }
            found.extend(read(harness, candidate, home)?);
        }
    }
    Ok(found)
}

pub fn view(grant: &HeldGrant, home: &Path) -> HeldGrantView {
    HeldGrantView {
        harness: grant.harness,
        provider: grant.provider,
        account: grant.account.clone(),
        expires_at_ms: grant.expires_at_ms,
        store: grant.harness.store(home).display().to_string(),
    }
}

/// The grants one harness holds for one provider.
pub fn read(
    harness: Harness,
    provider: &'static str,
    home: &Path,
) -> Result<Vec<HeldGrant>, String> {
    if !harness.providers().contains(&provider) {
        return Err(format!(
            "{} signs nothing in for `{provider}`; it holds grants for {}",
            harness.name(),
            harness.providers().join(", ")
        ));
    }
    let store = harness.store(home);
    match harness {
        Harness::Omp => omp::held(&store, provider),
        Harness::Claude => claude(&store, home),
        Harness::Codex => file(harness, provider, &store, |document| {
            let tokens = document.get("tokens")?;
            tokens
                .get("refresh_token")?
                .as_str()
                .filter(|token| !token.is_empty())?;
            let account = tokens
                .get("id_token")
                .and_then(Value::as_str)
                .and_then(jwt_email);
            Some((account, None))
        }),
        Harness::Kimi => file(harness, provider, &store, |document| {
            document
                .get("refresh_token")?
                .as_str()
                .filter(|token| !token.is_empty())?;
            let expires = document
                .get("expires_at")
                .and_then(Value::as_i64)
                .map(|seconds| seconds * millis());
            Some((None, expires))
        }),
    }
}

/// Claude Code keeps its grant in `~/.claude/.credentials.json`, and on
/// macOS in the Keychain under `Claude Code-credentials` instead. The file
/// is read when it is there; the Keychain only for the real home, since a
/// fixture home has no Keychain and the read may ask the person for
/// permission the first time.
fn claude(store: &Path, home: &Path) -> Result<Vec<HeldGrant>, String> {
    if store.is_file() {
        return file(Harness::Claude, "claude-code", store, claude_fields);
    }
    if !home_is_real(home) {
        return Ok(Vec::new());
    }
    let Some(text) = keychain::claude_code_credentials()? else {
        return Ok(Vec::new());
    };
    parse(Harness::Claude, "claude-code", &text, claude_fields)
}

fn claude_fields(document: &Value) -> Option<(Option<String>, Option<i64>)> {
    let oauth = document.get("claudeAiOauth")?;
    oauth
        .get("refreshToken")?
        .as_str()
        .filter(|token| !token.is_empty())?;
    Some((None, oauth.get("expiresAt").and_then(Value::as_i64)))
}

fn home_is_real(home: &Path) -> bool {
    std::env::var_os("HOME").is_some_and(|real| Path::new(&real) == home)
}

/// A harness that keeps one grant as one JSON file in Brama's own shape.
fn file(
    harness: Harness,
    provider: &'static str,
    store: &Path,
    fields: impl Fn(&Value) -> Option<(Option<String>, Option<i64>)>,
) -> Result<Vec<HeldGrant>, String> {
    if !store.is_file() {
        return Ok(Vec::new());
    }
    let text = Zeroizing::new(std::fs::read_to_string(store).map_err(|error| {
        format!(
            "{}'s store {} cannot be read: {error}",
            harness.name(),
            store.display()
        )
    })?);
    parse(harness, provider, &text, fields)
}

fn parse(
    harness: Harness,
    provider: &'static str,
    text: &str,
    fields: impl Fn(&Value) -> Option<(Option<String>, Option<i64>)>,
) -> Result<Vec<HeldGrant>, String> {
    let document: Value = serde_json::from_str(text).map_err(|_| {
        format!(
            "{}'s store is not the JSON document {} writes",
            harness.name(),
            harness.name()
        )
    })?;
    let Some((account, expires_at_ms)) = fields(&document) else {
        return Ok(Vec::new());
    };
    Ok(vec![HeldGrant {
        harness,
        provider,
        account,
        expires_at_ms,
        document: Zeroizing::new(document.to_string()),
    }])
}

/// The e-mail inside an OpenAI identity token, read for a label only: the
/// token is not verified here, and nothing decides on it.
fn jwt_email(token: &str) -> Option<String> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("email")?.as_str().map(str::to_owned)
}

pub(super) const MILLIS_PER_SECOND: i64 = 1000;

pub(super) fn millis() -> i64 {
    MILLIS_PER_SECOND
}

mod keychain {
    //! The macOS Keychain item Claude Code writes on this platform.

    use std::process::Command;

    use zeroize::Zeroizing;

    /// The JSON Claude Code stored, or None when the item is not there.
    /// macOS may ask the person to allow the read the first time.
    pub fn claude_code_credentials() -> Result<Option<Zeroizing<String>>, String> {
        if !cfg!(target_os = "macos") {
            return Ok(None);
        }
        let output = Command::new("/usr/bin/security")
            .args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ])
            .output()
            .map_err(|error| {
                format!("the Keychain could not be asked for Claude Code's grant: {error}")
            })?;
        if !output.status.success() {
            let refusal = String::from_utf8_lossy(&output.stderr);
            if refusal.contains("could not be found") {
                return Ok(None);
            }
            return Err(format!(
                "the Keychain would not give up Claude Code's grant: {}",
                refusal.trim()
            ));
        }
        Ok(Some(Zeroizing::new(
            String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        )))
    }
}
