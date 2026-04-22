//! The four CLI engine specializations. Each takes a decrypted donated
//! token plus a prompt and returns a `ModelResponse`. Which credential
//! location each engine expects:
//!
//! - `claude_code`: `CLAUDE_CODE_OAUTH_TOKEN` env var (no file needed).
//! - `codex`: `$HOME/.codex/auth.json` containing the JSON auth blob.
//! - `kimi`: `$HOME/.kimi/credentials/kimi-code.json` with the token JSON.
//! - `opencode`: reads `CLAUDE_CODE_OAUTH_TOKEN` in practice when backed by
//!   a Claude subscription; we pass it through as env.

use std::collections::HashMap;
use std::path::Path;

use tokio::fs;

use crate::subscription_dispatch::runtime::{
    build_prompt_from, run_cli, Sandbox,
};
use crate::types::{ModelRequest, ModelResponse};

pub async fn run_claude_code(
    request: &ModelRequest,
    token: &str,
) -> ModelResponse {
    let sandbox = match Sandbox::new() {
        Ok(s) => s,
        Err(e) => return ModelResponse::failure(&request.model, e.to_string()),
    };
    let prompt = build_prompt_from(&request.system, &request.messages);
    let mut env = HashMap::new();
    env.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), token.to_string());
    run_cli(
        &request.model,
        &["claude", "-p", &prompt],
        &sandbox,
        env,
    )
    .await
}

pub async fn run_codex(
    request: &ModelRequest,
    token_json: &str,
) -> ModelResponse {
    let sandbox = match Sandbox::new() {
        Ok(s) => s,
        Err(e) => return ModelResponse::failure(&request.model, e.to_string()),
    };
    let codex_dir = sandbox.home.join(".codex");
    if let Err(e) = fs::create_dir_all(&codex_dir).await {
        return ModelResponse::failure(&request.model, format!("mkdir: {e}"));
    }
    let auth_path = codex_dir.join("auth.json");
    if let Err(e) = fs::write(&auth_path, token_json).await {
        return ModelResponse::failure(&request.model, format!("write auth.json: {e}"));
    }
    let prompt = build_prompt_from(&request.system, &request.messages);
    run_cli(
        &request.model,
        &[
            "codex",
            "exec",
            "--dangerously-bypass-approvals-and-sandbox",
            "-C",
            sandbox.home.to_str().unwrap_or("."),
            &prompt,
        ],
        &sandbox,
        HashMap::new(),
    )
    .await
}

pub async fn run_kimi(
    request: &ModelRequest,
    token_json: &str,
) -> ModelResponse {
    let sandbox = match Sandbox::new() {
        Ok(s) => s,
        Err(e) => return ModelResponse::failure(&request.model, e.to_string()),
    };
    let kimi_dir = sandbox.home.join(".kimi").join("credentials");
    if let Err(e) = fs::create_dir_all(&kimi_dir).await {
        return ModelResponse::failure(&request.model, format!("mkdir: {e}"));
    }
    let creds_path = kimi_dir.join("kimi-code.json");
    if let Err(e) = fs::write(&creds_path, token_json).await {
        return ModelResponse::failure(&request.model, format!("write kimi creds: {e}"));
    }
    let _ = chmod_private(&creds_path).await;
    let prompt = build_prompt_from(&request.system, &request.messages);
    run_cli(
        &request.model,
        &[
            "kimi",
            "--print",
            "--yolo",
            "--work-dir",
            sandbox.home.to_str().unwrap_or("."),
            "-p",
            &prompt,
        ],
        &sandbox,
        HashMap::new(),
    )
    .await
}

pub async fn run_opencode(
    request: &ModelRequest,
    token: &str,
) -> ModelResponse {
    let sandbox = match Sandbox::new() {
        Ok(s) => s,
        Err(e) => return ModelResponse::failure(&request.model, e.to_string()),
    };
    let prompt = build_prompt_from(&request.system, &request.messages);
    let mut env = HashMap::new();
    env.insert("CLAUDE_CODE_OAUTH_TOKEN".into(), token.to_string());
    run_cli(
        &request.model,
        &["opencode", "run", &prompt],
        &sandbox,
        env,
    )
    .await
}

async fn chmod_private(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        fs::set_permissions(path, perms).await?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}
