//! The coding-agent harness a pool account is signed into, through the
//! harness's own documented login: which accounts it already holds, and one
//! login run whose authorization Weles completes.
//!
//! OMP's `omp login anthropic` prints the provider's authorize URL and reads
//! "the final redirect URL or authorization code" on stdin when the browser
//! cannot reach the machine (omp://providers.md, "omp login"). The
//! authorization is bound to the verifier only that
//! login process holds, so what crosses machines is a one-time redirect, never
//! a grant: OMP mints and refreshes its own pair, and Brama's pair is never
//! shared with it.

use std::collections::BTreeSet;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// A coding-agent harness on this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Harness {
    Omp,
}

impl Harness {
    pub fn name(self) -> &'static str {
        match self {
            Self::Omp => "omp",
        }
    }

    /// The harness's own id for a Brama provider.
    pub(super) fn provider_id(self, provider: &str) -> Option<&'static str> {
        match (self, provider) {
            (Self::Omp, "claude-code") => Some("anthropic"),
            _ => None,
        }
    }
}

/// The accounts the harness holds for its provider `harness_provider`, as
/// `omp usage accounts` prints them: a provider line, then one indented
/// `email:<address>|org:<id>` line per account. `None` when the harness is
/// not installed on this machine: there is nothing here to sign in, which is
/// the answer on every host the CLI is installed on but agents do not run.
pub(super) async fn held_accounts(
    harness: Harness,
    harness_provider: &str,
) -> Result<Option<BTreeSet<String>>, String> {
    let output = match tokio::process::Command::new(harness.name())
        .args(["usage", "accounts"])
        .output()
        .await
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "`{} usage accounts` did not start: {error}",
                harness.name()
            ))
        }
    };
    if !output.status.success() {
        return Err(format!(
            "`{} usage accounts` exited {}: {}",
            harness.name(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut current = None;
    let mut held = BTreeSet::new();
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) {
            current = Some(line.trim().to_owned());
            continue;
        }
        if current.as_deref() != Some(harness_provider) {
            continue;
        }
        let identity = line.trim();
        if let Some(email) = identity
            .strip_prefix("email:")
            .and_then(|rest| rest.split('|').next())
        {
            held.insert(email.to_lowercase());
        }
    }
    Ok(Some(held))
}

/// One running `<harness> login <provider>`, stopped at the authorize URL it
/// printed and waiting for the redirect on stdin.
pub(super) struct Login {
    child: tokio::process::Child,
    pub authorize_url: String,
    transcript: Vec<String>,
}

impl Login {
    /// Start the login and read its output up to the authorize URL.
    pub(super) async fn start(harness: Harness, harness_provider: &str) -> Result<Self, String> {
        let mut child = tokio::process::Command::new(harness.name())
            .args(["login", harness_provider])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| {
                format!("`{} login {harness_provider}` did not start: {error}", harness.name())
            })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            format!("`{} login {harness_provider}` gave no stdout", harness.name())
        })?;
        let mut lines = BufReader::new(stdout).lines();
        let mut transcript = Vec::new();
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|error| format!("reading `{} login`: {error}", harness.name()))?
        {
            let trimmed = line.trim().to_owned();
            transcript.push(trimmed.clone());
            if trimmed.starts_with("https://") && trimmed.contains("/oauth/authorize?") {
                // The rest of the output is read when the login ends.
                child.stdout = None;
                tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
                return Ok(Self {
                    child,
                    authorize_url: trimmed,
                    transcript,
                });
            }
        }
        Err(format!(
            "`{} login {harness_provider}` ended without printing an authorize URL; it printed: {}",
            harness.name(),
            transcript.join(" | ")
        ))
    }

    /// Hand the provider's redirect to the login and wait for it to end.
    pub(super) async fn finish(mut self, redirect: &str) -> Result<(), String> {
        let mut stdin = self
            .child
            .stdin
            .take()
            .ok_or_else(|| "the harness login gave no stdin".to_string())?;
        stdin
            .write_all(format!("{redirect}\n").as_bytes())
            .await
            .map_err(|error| format!("writing the redirect to the harness login: {error}"))?;
        drop(stdin);
        let output = self
            .child
            .wait_with_output()
            .await
            .map_err(|error| format!("waiting for the harness login: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        Err(format!(
            "the harness login exited {} after the redirect was handed to it: {} {}",
            output.status,
            self.transcript.join(" | "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}
