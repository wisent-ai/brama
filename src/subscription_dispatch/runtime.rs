//! Shared subprocess runner for the four CLI engines.
//!
//! Each subscription dispatch gets its own tempdir mounted as $HOME so that
//! concurrent requests don't stomp on one another's credential files. ANSI
//! escape codes are stripped from stdout before returning.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use thiserror::Error;
use tokio::process::Command;

use crate::types::ModelResponse;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("tempdir creation failed: {0}")]
    TempDir(String),
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("cli exited with status {status}: {stderr}")]
    NonZero { status: i32, stderr: String },
    #[error("i/o: {0}")]
    Io(String),
}

/// Per-request sandbox: a fresh HOME directory that the caller is expected to
/// populate with credential files before `run_cli` is invoked.
pub struct Sandbox {
    pub home: PathBuf,
    _guard: tempfile::TempDir,
}

impl Sandbox {
    pub fn new() -> Result<Self, RuntimeError> {
        let td = tempfile::tempdir()
            .map_err(|e| RuntimeError::TempDir(e.to_string()))?;
        Ok(Self {
            home: td.path().to_path_buf(),
            _guard: td,
        })
    }
}

/// Extract the last user message's content as the prompt string passed to
/// the CLI. System message (if any) is prepended.
pub fn build_prompt_from(
    system: &Option<String>,
    messages: &[crate::types::Message],
) -> String {
    let mut out = String::new();
    if let Some(s) = system {
        out.push_str(s);
        out.push_str("\n\n");
    }
    for m in messages {
        if m.role == "user" || m.role == "assistant" {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("{}: {}", m.role, m.content));
        }
    }
    out
}

/// Execute `argv` in `sandbox.home` as HOME, with `extra_env` merged in,
/// wrapping stdout into a ModelResponse.
pub async fn run_cli(
    model: &str,
    argv: &[&str],
    sandbox: &Sandbox,
    extra_env: HashMap<String, String>,
) -> ModelResponse {
    if argv.is_empty() {
        return ModelResponse::failure(model, "empty argv for CLI dispatch".into());
    }
    let start = Instant::now();
    let mut cmd = Command::new(argv[0]);
    cmd.args(&argv[1..])
        .env("HOME", &sandbox.home)
        .env("PATH", std::env::var("PATH").unwrap_or_default());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.current_dir(&sandbox.home);

    let output = match cmd.output().await {
        Ok(o) => o,
        Err(e) => {
            return ModelResponse::failure(model, format!("spawn: {e}"));
        }
    };
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return ModelResponse::failure(
            model,
            format!(
                "exit {}: {}",
                output.status.code().unwrap_or(-1),
                stderr.chars().take(2000).collect::<String>()
            ),
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let clean = strip_ansi(&stdout);
    ModelResponse {
        content: clean,
        model: model.to_string(),
        input_tokens: 0,
        output_tokens: 0,
        latency_ms: elapsed,
        cost: 0.0,
        success: true,
        error: None,
        tool_calls: None,
    }
}

/// Strip ANSI escape sequences from a string. CLIs such as claude print
/// terminal colour codes; we don't want those in the OpenAI-shaped response.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if let Some(&n) = chars.peek() {
                if n == '[' || n == ']' {
                    chars.next();
                    for inner in chars.by_ref() {
                        if inner.is_ascii_alphabetic() || inner == '\x07' {
                            break;
                        }
                    }
                    continue;
                }
            }
        }
        out.push(c);
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_csi() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(
            strip_ansi("plain text"),
            "plain text"
        );
        assert_eq!(
            strip_ansi("\x1b[1;32mok\x1b[0m  "),
            "ok"
        );
    }
}
