//! The four harness stores, each the way its own harness writes it, and
//! the real binary run against them.
//!
//! `omp` holds the given enabled Claude accounts, one disabled one and one
//! codex account; Codex CLI, Kimi Code and Claude Code each hold one grant.
//! The grants are not real, so a provider refuses the proof — what the
//! stories assert is what Brama did before that answer.

use super::*;

pub(crate) const PRIMARY: &str = "lbartoszcze@wisent.ai";
pub(crate) const SECOND: &str = "controlyourai@gmail.com";
pub(crate) const LAPSED: &str = "lukasz.bartoszcze@wisent.ai";
pub(crate) const GRANT: &str = "/v1/admin/subscription-pool/grant";

/// One omp row's document, as omp writes it.
pub(crate) fn omp_row(email: &str) -> String {
    json!({
        "access": format!("sk-oat-{email}"),
        "refresh": format!("sk-ort-{email}"),
        "expires": 1_789_400_000_000_i64,
        "accountId": format!("acct-{email}"),
        "email": email,
        "orgName": "Wisent",
    })
    .to_string()
}

/// A home directory with every harness store, each the way its harness
/// writes it. `omp` holds the given enabled Claude accounts, one disabled
/// one, and one codex account; Codex CLI, Kimi Code and Claude Code each
/// hold one grant.
pub(crate) fn home_with_every_harness(
    directory: &TestDirectory,
    omp_claude: &[&str],
) -> std::path::PathBuf {
    let home = directory.path().join("home");
    std::fs::create_dir_all(home.join(".omp/agent")).expect("omp's directory");
    let store = Connection::open(home.join(".omp/agent/agent.db")).expect("create omp's store");
    store
        .execute_batch(
            "CREATE TABLE auth_credentials (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                provider TEXT NOT NULL,
                credential_type TEXT NOT NULL,
                data TEXT NOT NULL,
                disabled_cause TEXT DEFAULT NULL,
                identity_key TEXT DEFAULT NULL,
                created_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER)),
                updated_at INTEGER NOT NULL DEFAULT (CAST(strftime('%s','now') AS INTEGER))
            );",
        )
        .expect("omp's schema");
    let mut insert = store
        .prepare(
            "INSERT INTO auth_credentials (provider, credential_type, data, disabled_cause, identity_key, updated_at)
             VALUES (?1, 'oauth', ?2, ?3, ?4, ?5)",
        )
        .expect("prepare");
    for (index, email) in omp_claude.iter().enumerate() {
        insert
            .execute(rusqlite::params![
                "anthropic",
                omp_row(email),
                Option::<&str>::None,
                format!("email:{email}"),
                1_789_300_000 + index as i64
            ])
            .expect("an enabled omp row");
    }
    insert
        .execute(rusqlite::params![
            "anthropic",
            omp_row(LAPSED),
            Some("deleted by user"),
            format!("email:{LAPSED}"),
            1_789_300_999
        ])
        .expect("a disabled omp row");
    insert
        .execute(rusqlite::params![
            "openai-codex",
            omp_row("lukasz@wisent.com"),
            Option::<&str>::None,
            "email:lukasz@wisent.com",
            1_789_300_500
        ])
        .expect("omp's codex row");
    drop(insert);
    std::fs::create_dir_all(home.join(".codex")).expect("codex's directory");
    std::fs::write(
        home.join(".codex/auth.json"),
        json!({
            "OPENAI_API_KEY": null,
            "auth_mode": "chatgpt",
            "tokens": {"access_token": "sk-oat-codex-cli", "refresh_token": "sk-ort-codex-cli", "account_id": "acct-codex-cli",
                       "id_token": "eyJhbGciOiJub25lIn0.eyJlbWFpbCI6ImNvZGV4QHdpc2VudC5haSJ9."},
            "last_refresh": "2026-09-13T00:00:00Z",
        })
        .to_string(),
    )
    .expect("codex's store");
    std::fs::create_dir_all(home.join(".kimi-code/credentials")).expect("kimi's directory");
    std::fs::write(
        home.join(".kimi-code/credentials/kimi-code.json"),
        json!({"access_token": "sk-oat-kimi", "refresh_token": "sk-ort-kimi", "expires_at": 1_789_400_000_i64, "token_type": "Bearer"}).to_string(),
    )
    .expect("kimi's store");
    std::fs::create_dir_all(home.join(".claude")).expect("claude's directory");
    std::fs::write(
        home.join(".claude/.credentials.json"),
        json!({"claudeAiOauth": {"accessToken": "sk-oat-claude-code", "refreshToken": "sk-ort-claude-code", "expiresAt": 1_789_400_000_000_i64, "scopes": ["user:inference"]}}).to_string(),
    )
    .expect("claude's store");
    home
}

pub(crate) fn brama(
    vault: &SkarbiecVault,
    args: &[&str],
    stdin: Option<&str>,
) -> (i32, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    command
        .args(args)
        .env("ENTITLEMENTS_ROUTER_BIN", vault.router());
    for (name, value) in vault.environment() {
        command.env(name, value);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("run the real brama binary");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.unwrap_or_default().as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("collect output");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

pub(crate) fn import(
    vault: &SkarbiecVault,
    home: &Path,
    provider: &str,
    extra: &[&str],
) -> (i32, String, String) {
    let home = home.to_str().expect("utf-8 path");
    let mut args = vec![
        "subscription",
        "import",
        provider,
        "--subscription-id",
        "pool-agent",
        "--reason",
        "story",
        "--home",
        home,
    ];
    args.extend_from_slice(extra);
    brama(vault, &args, None)
}

pub(crate) fn stored(vault: &SkarbiecVault, item: &str) -> Value {
    serde_json::from_str(
        vault.document_of(item)["fields"]["value"]
            .as_str()
            .expect("a stored credential"),
    )
    .expect("JSON")
}
