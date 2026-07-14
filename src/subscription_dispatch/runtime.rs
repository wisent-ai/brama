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

const CODEX_COVERAGE_HOOKS_ENV: &str = "BRAMA_CODEX_COVERAGE_HOOKS";
const HOOK_CONFIG_GUARD_FILE: &str = "block_hook_config_edits_without_consent.py";
const HOOK_CONFIG_GUARD_ID: &str = "block-hook-config-edits-without-consent";
pub const CODEX_HOOK_COVERAGE_LOG: &str = "hook-coverage.jsonl";
const CODEX_FULL_HOOKS_PATH_ENV: &str = "BRAMA_CODEX_FULL_HOOKS_PATH";
const HOOK_CONFIG_GUARD_SOURCE: &str =
    include_str!("codex_hooks/block_hook_config_edits_without_consent.py");

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
            let url = match part
                .get("image_url")
                .and_then(|iu| iu.get("url"))
                .and_then(|u| u.as_str())
            {
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
                    let ext = meta
                        .split(';')
                        .next()
                        .unwrap_or("image/png")
                        .strip_prefix("image/")
                        .unwrap_or("png")
                        .to_string();
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
pub fn build_prompt_from(system: &Option<String>, messages: &[crate::types::Message]) -> String {
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

pub fn codex_coverage_hooks_enabled() -> bool {
    matches!(
        std::env::var(CODEX_COVERAGE_HOOKS_ENV)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on" | "full-local"
    )
}

fn codex_full_local_coverage_hooks_enabled() -> bool {
    std::env::var(CODEX_COVERAGE_HOOKS_ENV)
        .unwrap_or_default()
        .eq_ignore_ascii_case("full-local")
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn codex_runtime_event(event: &str, matcher: Option<&str>) -> String {
    match event {
        "Stop" => "stop".to_string(),
        "UserPromptSubmit" => "user_prompt_submit".to_string(),
        "PostToolUse" => {
            let tool = matcher.unwrap_or("").to_ascii_lowercase();
            if tool.contains("bash") {
                "post_tool_use:bash".to_string()
            } else {
                "post_tool_use".to_string()
            }
        }
        "SessionStart" => {
            let matcher = matcher.unwrap_or("").to_ascii_lowercase();
            if matcher.contains("compact") {
                "session_start:compact".to_string()
            } else {
                "session_start".to_string()
            }
        }
        "PreToolUse" => "pre_tool_use".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

fn codex_full_wrapper_script(
    command: &str,
    default_event: &str,
    coverage_log: &Path,
) -> Result<String, RuntimeError> {
    let command_json = serde_json::to_string(command)
        .map_err(|e| RuntimeError::Io(format!("serialize codex hook command: {e}")))?;
    Ok(format!(
        concat!(
            "#!/bin/sh\n",
            "payload=$(cat)\n",
            "event={default_event}\n",
            "case \"$event\" in\n",
            "  pre_tool_use)\n",
            "    tool=$(printf '%s' \"$payload\" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(str(d.get(\"tool_name\") or d.get(\"tool\") or d.get(\"name\") or \"\").lower())' 2>/dev/null || true)\n",
            "    case \"$tool\" in\n",
            "      bash|shell|exec|exec_command|run_command|runcommands|terminal) event='pre_tool_use:bash' ;;\n",
            "      read) event='pre_tool_use:read' ;;\n",
            "      write) event='pre_tool_use:write' ;;\n",
            "      edit|apply_patch|applypatch|functions.apply_patch|multiedit) event='pre_tool_use:edit' ;;\n",
            "      notebook|edit_notebook|notebookedit) event='pre_tool_use:notebook' ;;\n",
            "      wait|monitor|schedulewakeup|schedule_wakeup) event='pre_tool_use:wait' ;;\n",
            "      *) event=\"pre_tool_use:$tool\" ;;\n",
            "    esac\n",
            "    ;;\n",
            "esac\n",
            "cmd_json={command_json}\n",
            "printf '%s' \"$payload\" | sh -c {command}\n",
            "code=$?\n",
            "printf '{{\"event\":\"%s\",\"command\":%s,\"code\":%s}}\\n' \"$event\" \"$cmd_json\" \"$code\" >> {coverage_log}\n",
            "exit \"$code\"\n"
        ),
        default_event = shell_single_quote(default_event),
        command_json = shell_single_quote(&command_json),
        command = shell_single_quote(command),
        coverage_log = shell_single_quote(&coverage_log.display().to_string())
    ))
}

fn wrapper_file_stem(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

async fn chmod_executable(path: &Path, context: &str) -> Result<(), RuntimeError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(path, permissions)
            .await
            .map_err(|e| RuntimeError::Io(format!("chmod {context}: {e}")))?;
    }
    Ok(())
}

async fn materialize_codex_full_local_coverage_hooks(
    codex_dir: &Path,
    hooks_dir: &Path,
) -> Result<(), RuntimeError> {
    let hooks_path = std::env::var(CODEX_FULL_HOOKS_PATH_ENV).map_err(|_| {
        RuntimeError::Io(format!(
            "{CODEX_FULL_HOOKS_PATH_ENV} is required when {CODEX_COVERAGE_HOOKS_ENV}=full-local"
        ))
    })?;
    let hooks_source = tokio::fs::read_to_string(&hooks_path)
        .await
        .map_err(|e| RuntimeError::Io(format!("read full local codex hooks: {e}")))?;
    let mut config: serde_json::Value = serde_json::from_str(&hooks_source)
        .map_err(|e| RuntimeError::Io(format!("parse full local codex hooks: {e}")))?;
    let coverage_log = codex_dir.join(CODEX_HOOK_COVERAGE_LOG);

    let events = config
        .get_mut("hooks")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            RuntimeError::Io("full local codex hooks missing hooks object".to_string())
        })?;

    for (event, entries_value) in events {
        let entries = entries_value.as_array_mut().ok_or_else(|| {
            RuntimeError::Io(format!("full local codex event {event} is not an array"))
        })?;
        for (entry_index, entry) in entries.iter_mut().enumerate() {
            let matcher = entry
                .get("matcher")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let default_event = codex_runtime_event(event, matcher.as_deref());
            let hooks = entry
                .get_mut("hooks")
                .and_then(serde_json::Value::as_array_mut)
                .ok_or_else(|| {
                    RuntimeError::Io(format!(
                        "full local codex event {event} entry {entry_index} missing hooks array"
                    ))
                })?;
            for (hook_index, hook) in hooks.iter_mut().enumerate() {
                let command = hook
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        RuntimeError::Io(format!(
                            "full local codex event {event} entry {entry_index} hook {hook_index} missing command"
                        ))
                    })?
                    .to_string();
                let wrapper_name = format!(
                    "full_{}_{}_{}.sh",
                    wrapper_file_stem(event),
                    entry_index,
                    hook_index
                );
                let wrapper_path = hooks_dir.join(wrapper_name);
                let wrapper = codex_full_wrapper_script(&command, &default_event, &coverage_log)?;
                tokio::fs::write(&wrapper_path, wrapper)
                    .await
                    .map_err(|e| {
                        RuntimeError::Io(format!("write full local codex wrapper: {e}"))
                    })?;
                chmod_executable(&wrapper_path, "full local codex wrapper").await?;
                hook["command"] = serde_json::Value::String(wrapper_path.display().to_string());
            }
        }
    }

    let hooks_json = serde_json::to_vec_pretty(&config)
        .map_err(|e| RuntimeError::Io(format!("serialize full local codex hooks.json: {e}")))?;
    tokio::fs::write(codex_dir.join("hooks.json"), hooks_json)
        .await
        .map_err(|e| RuntimeError::Io(format!("write full local codex hooks.json: {e}")))?;
    Ok(())
}

pub async fn materialize_codex_coverage_hooks(codex_dir: &Path) -> Result<(), RuntimeError> {
    let hooks_dir = codex_dir.join("hooks");
    tokio::fs::create_dir_all(&hooks_dir)
        .await
        .map_err(|e| RuntimeError::Io(format!("mkdir codex hooks: {e}")))?;

    if codex_full_local_coverage_hooks_enabled() {
        return materialize_codex_full_local_coverage_hooks(codex_dir, &hooks_dir).await;
    }

    let guard_path = hooks_dir.join(HOOK_CONFIG_GUARD_FILE);
    tokio::fs::write(&guard_path, HOOK_CONFIG_GUARD_SOURCE)
        .await
        .map_err(|e| RuntimeError::Io(format!("write codex hook guard: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(&guard_path, permissions)
            .await
            .map_err(|e| RuntimeError::Io(format!("chmod codex hook guard: {e}")))?;
    }

    let coverage_log = codex_dir.join(CODEX_HOOK_COVERAGE_LOG);
    let wrapper_path = hooks_dir.join("block_hook_config_edits_without_consent_wrapper.sh");
    let wrapper = format!(
        concat!(
            "#!/bin/sh\n",
            "payload=$(cat)\n",
            "tool=$(printf '%s' \"$payload\" | python3 -c 'import json,sys; d=json.load(sys.stdin); print(str(d.get(\"tool_name\") or d.get(\"tool\") or d.get(\"name\") or \"\").lower())' 2>/dev/null || true)\n",
            "case \"$tool\" in\n",
            "  bash|shell|exec|exec_command|run_command|runcommands|terminal) event='pre_tool_use:bash' ;;\n",
            "  read) event='pre_tool_use:read' ;;\n",
            "  write) event='pre_tool_use:write' ;;\n",
            "  edit|apply_patch|applypatch|functions.apply_patch|multiedit) event='pre_tool_use:edit' ;;\n",
            "  notebook|edit_notebook|notebookedit) event='pre_tool_use:notebook' ;;\n",
            "  wait) event='pre_tool_use:wait' ;;\n",
            "  *) event=\"pre_tool_use:$tool\" ;;\n",
            "esac\n",
            "printf '%s' \"$payload\" | python3 '{}'\n",
            "code=$?\n",
            "printf '{{\"event\":\"%s\",\"hook_id\":\"{}\",\"code\":%s}}\\n' \"$event\" \"$code\" >> '{}'\n",
            "exit \"$code\"\n"
        ),
        guard_path.display(),
        HOOK_CONFIG_GUARD_ID,
        coverage_log.display()
    );
    tokio::fs::write(&wrapper_path, wrapper)
        .await
        .map_err(|e| RuntimeError::Io(format!("write codex hook wrapper: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::Permissions::from_mode(0o755);
        tokio::fs::set_permissions(&wrapper_path, permissions)
            .await
            .map_err(|e| RuntimeError::Io(format!("chmod codex hook wrapper: {e}")))?;
    }

    let command = wrapper_path.display().to_string();
    let config = serde_json::json!({
        "hooks": {
            "PreToolUse": [
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": command,
                            "timeout": 10,
                            "statusMessage": "Checking hook config edit consent"
                        }
                    ]
                }
            ]
        }
    });
    let hooks_json = serde_json::to_vec_pretty(&config)
        .map_err(|e| RuntimeError::Io(format!("serialize codex hooks.json: {e}")))?;
    tokio::fs::write(codex_dir.join("hooks.json"), hooks_json)
        .await
        .map_err(|e| RuntimeError::Io(format!("write codex hooks.json: {e}")))?;

    for path in [&guard_path, &wrapper_path] {
        if !path.exists() {
            return Err(RuntimeError::Io(format!(
                "codex hook command target missing after materialization: {}",
                path.display()
            )));
        }
    }
    Ok(())
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
    use std::io::Write as _;
    use std::process::Stdio;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarRestore {
        original: Option<std::ffi::OsString>,
    }

    impl Drop for EnvVarRestore {
        fn drop(&mut self) {
            match self.original.take() {
                Some(original) => std::env::set_var(CODEX_COVERAGE_HOOKS_ENV, original),
                None => std::env::remove_var(CODEX_COVERAGE_HOOKS_ENV),
            }
        }
    }

    fn with_codex_coverage_hooks_env<T>(value: Option<&str>, exercise: impl FnOnce() -> T) -> T {
        let _lock = ENV_LOCK.lock().expect("env mutex poisoned");
        let _restore = EnvVarRestore {
            original: std::env::var_os(CODEX_COVERAGE_HOOKS_ENV),
        };
        match value {
            Some(value) => std::env::set_var(CODEX_COVERAGE_HOOKS_ENV, value),
            None => std::env::remove_var(CODEX_COVERAGE_HOOKS_ENV),
        }

        exercise()
    }

    fn command_paths(command: &str) -> Vec<PathBuf> {
        command
            .split_whitespace()
            .filter_map(|token| {
                let token = token.trim_matches(['"', '\'']);
                token.starts_with('/').then(|| PathBuf::from(token))
            })
            .collect()
    }

    #[test]
    fn strip_ansi_removes_csi() {
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi("plain text"), "plain text");
        assert_eq!(strip_ansi("\x1b[1;32mok\x1b[0m  "), "ok");
    }

    #[test]
    fn codex_coverage_hooks_enabled_requires_explicit_truthy_flag() {
        with_codex_coverage_hooks_env(None, || {
            assert!(!codex_coverage_hooks_enabled());
        });

        for value in ["", "0", "false", "no", "off", "enabled"] {
            with_codex_coverage_hooks_env(Some(value), || {
                assert!(
                    !codex_coverage_hooks_enabled(),
                    "{value:?} must not enable Codex coverage hooks"
                );
            });
        }

        for value in ["1", "true", "TRUE", "yes", "on"] {
            with_codex_coverage_hooks_env(Some(value), || {
                assert!(
                    codex_coverage_hooks_enabled(),
                    "{value:?} must enable Codex coverage hooks"
                );
            });
        }
    }

    #[tokio::test]
    async fn materialize_codex_coverage_hooks_writes_sandbox_local_bash_guard() {
        let codex_home = tempfile::tempdir().expect("create temp codex home");

        materialize_codex_coverage_hooks(codex_home.path())
            .await
            .expect("materialize Codex coverage hooks");

        let hooks_json_path = codex_home.path().join("hooks.json");
        let hooks_json = tokio::fs::read(&hooks_json_path)
            .await
            .expect("read generated hooks.json");
        let config: serde_json::Value =
            serde_json::from_slice(&hooks_json).expect("generated hooks.json is valid JSON");

        let root = config.as_object().expect("hooks.json root is an object");
        assert_eq!(
            root.keys().collect::<Vec<_>>(),
            vec!["hooks"],
            "hooks.json must only contain the hooks root"
        );

        let hooks = config
            .get("hooks")
            .and_then(serde_json::Value::as_object)
            .expect("hooks root is an object");
        assert_eq!(
            hooks.keys().collect::<Vec<_>>(),
            vec!["PreToolUse"],
            "only PreToolUse hooks should be materialized"
        );

        let pre_tool_use = hooks
            .get("PreToolUse")
            .and_then(serde_json::Value::as_array)
            .expect("PreToolUse is an array");
        assert_eq!(pre_tool_use.len(), 1, "only one PreToolUse entry exists");

        let pre_tool_use_entry = pre_tool_use[0]
            .as_object()
            .expect("PreToolUse entry is an object");
        assert!(
            !pre_tool_use_entry.contains_key("matcher"),
            "coverage guard must run for any Codex PreToolUse tool name"
        );

        let command_hooks = pre_tool_use_entry
            .get("hooks")
            .and_then(serde_json::Value::as_array)
            .expect("PreToolUse entry contains command hooks");
        assert_eq!(
            command_hooks.len(),
            1,
            "only one PreToolUse command hook exists"
        );

        let command_hook = command_hooks[0]
            .as_object()
            .expect("PreToolUse command hook is an object");
        assert_eq!(
            command_hook.get("type").and_then(serde_json::Value::as_str),
            Some("command")
        );

        let command = command_hook
            .get("command")
            .and_then(serde_json::Value::as_str)
            .expect("PreToolUse command hook contains a command string");

        let paths = command_paths(command);
        assert_eq!(
            paths.len(),
            1,
            "hook command should point at one wrapper path"
        );
        let wrapper_path = &paths[0];
        assert!(
            wrapper_path.starts_with(codex_home.path()),
            "wrapper path {} must stay under temp Codex home {}",
            wrapper_path.display(),
            codex_home.path().display()
        );
        assert!(
            wrapper_path.exists(),
            "wrapper path {} must exist after materialization",
            wrapper_path.display()
        );

        let guard_path = codex_home.path().join("hooks").join(HOOK_CONFIG_GUARD_FILE);
        assert!(
            guard_path.exists(),
            "embedded guard script must be materialized"
        );

        let mut child = std::process::Command::new(wrapper_path)
            .env_remove("DEVICE_HOOK_EDIT_APPROVED")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn guard wrapper");
        child
            .stdin
            .as_mut()
            .expect("guard stdin is piped")
            .write_all(
                br#"{"tool_name":"exec_command","tool_input":{"command":"git config core.hooksPath .githooks"}}"#,
            )
            .expect("write Codex shell payload");
        let output = child.wait_with_output().expect("wait for guard wrapper");

        assert_eq!(
            output.status.code(),
            Some(2),
            "guard must block hook path configuration attempts"
        );
        let stderr = String::from_utf8(output.stderr).expect("guard stderr is utf-8");
        assert!(
            stderr.contains("BLOCKED: hook source/config changes are device-level protected"),
            "guard stderr should explain the protected hook block, got {stderr:?}"
        );
        let coverage = tokio::fs::read_to_string(codex_home.path().join(CODEX_HOOK_COVERAGE_LOG))
            .await
            .expect("coverage log is written");
        assert!(
            coverage.contains("\"event\":\"pre_tool_use:bash\"")
                && coverage.contains("\"hook_id\":\"block-hook-config-edits-without-consent\"")
                && coverage.contains("\"code\":2"),
            "coverage log should record the wrapper result, got {coverage:?}"
        );
    }
}
