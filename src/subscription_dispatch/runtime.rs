//! Shared subprocess runner for the four CLI engines.
//!
//! Each subscription dispatch gets its own tempdir mounted as $HOME so that
//! concurrent requests don't stomp on one another's credential files. ANSI
//! escape codes are stripped from stdout before returning.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use base64::Engine as _;
use thiserror::Error;
use tokio::process::Command;

use crate::types::ModelResponse;

/// Extract image_url parts from multimodal messages, base64-decode them,
/// write each to `sandbox_home/img_<n>.<ext>`, and return the list of
/// filenames (relative to sandbox_home). Lets the claude CLI Read tool
/// (pre-approved via --settings) pick them up via @file references.
pub async fn materialize_images(
    messages: &[crate::types::Message],
    sandbox_home: &Path,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut n = 0;
    for m in messages {
        let parts = match m.content.as_array() {
            Some(a) => a,
            None => continue,
        };
        for part in parts {
            if part.get("type").and_then(|v| v.as_str()) != Some("image_url") {
                continue;
            }
            let url = match part.get("image_url").and_then(|iu| iu.get("url")).and_then(|u| u.as_str()) {
                Some(s) => s,
                None => continue,
            };
            let (ext, payload) = match url.strip_prefix("data:") {
                Some(rest) => {
                    let comma = match rest.find(',') {
                        Some(i) => i,
                        None => continue,
                    };
                    let (meta, b64_with_comma) = rest.split_at(comma);
                    let ext = meta.split(';').next().unwrap_or("image/png")
                        .strip_prefix("image/").unwrap_or("png").to_string();
                    (ext, b64_with_comma.trim_start_matches(','))
                }
                None => continue,
            };
            let bytes = match base64::engine::general_purpose::STANDARD.decode(payload) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let filename = format!("img_{n}.{ext}");
            let path = sandbox_home.join(&filename);
            if tokio::fs::write(&path, &bytes).await.is_ok() {
                out.push(filename);
                n += 1;
            }
        }
    }
    out
}

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
        // Cloud Run and some sandboxed environments allow writing under /tmp but
        // refuse to execute helper binaries from there (e.g. Codex CLI validates
        // that its home directory is not a world-writable tmpfs). Prefer
        // /var/tmp when available and fall back to the default temp dir.
        let candidates = ["/var/tmp", "/tmp"];
        let mut last_err = None;
        for base in &candidates {
            let base_path = Path::new(base);
            if base_path.exists() && base_path.is_dir() {
                match tempfile::Builder::new()
                    .prefix("router-sandbox-")
                    .tempdir_in(base_path)
                {
                    Ok(td) => {
                        return Ok(Self {
                            home: td.path().to_path_buf(),
                            _guard: td,
                        });
                    }
                    Err(e) => last_err = Some(e),
                }
            }
        }
        let td = tempfile::tempdir().map_err(|e| {
            RuntimeError::TempDir(match last_err {
                Some(le) => format!("{}; fallback: {}", le, e),
                None => e.to_string(),
            })
        })?;
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
            out.push_str(&format!("{}: {}", m.role, m.content_text()));
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

    let argv_joined = argv.join(" ");
    tracing::info!(target: "subscription_dispatch", "exec: {}", argv_joined);
    let output = match cmd.output().await {
        Ok(o) => o,
        Err(e) => {
            return ModelResponse::failure(model, format!("spawn: {e}"));
        }
    };
    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        tracing::error!(
            target: "subscription_dispatch",
            "cli exit {}: argv={:?} stdout={:?} stderr={:?}",
            output.status.code().unwrap_or(-1),
            argv,
            stdout.chars().take(4000).collect::<String>(),
            stderr.chars().take(4000).collect::<String>(),
        );
        let merged = format!(
            "stderr: {} | stdout: {}",
            stderr.trim().chars().take(1500).collect::<String>(),
            strip_ansi(&stdout).chars().take(1500).collect::<String>(),
        );
        return ModelResponse::failure(
            model,
            format!("exit {}: {}", output.status.code().unwrap_or(-1), merged),
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
