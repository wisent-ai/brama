//! Taking the grants the harnesses on this machine already hold.
//!
//! Four harnesses sign accounts in and each keeps the grant its own way:
//! `omp` in a SQLite store, Claude Code in a credentials file, Codex CLI in
//! `auth.json`, Kimi Code in a credentials file. These stories build every
//! one of them the way that harness writes it, under an isolated home, and
//! drive the real binary against them and a real isolated vault. The grants
//! are not real, so the provider refuses the proof; what the stories assert
//! is what Brama did before that answer - which grant it took, what it
//! stored and in what shape - and how it refuses when a store cannot answer.

#[path = "../support/gateway.rs"]
mod gateway;
#[path = "../support/mod.rs"]
mod support;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use reqwest::Method;
use rusqlite::Connection;
use serde_json::{json, Value};

use gateway::{refusal, Gateway, AGENT, CONSOLE_BEARER};
use support::{SkarbiecVault, TestDirectory};

const PRIMARY: &str = "lbartoszcze@wisent.ai";
const SECOND: &str = "controlyourai@gmail.com";
const LAPSED: &str = "lukasz.bartoszcze@wisent.ai";
const GRANT: &str = "/v1/admin/subscription-pool/grant";

/// One omp row's document, as omp writes it.
fn omp_row(email: &str) -> String {
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
fn home_with_every_harness(directory: &TestDirectory, omp_claude: &[&str]) -> std::path::PathBuf {
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

fn brama(vault: &SkarbiecVault, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
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

fn import(
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

fn stored(vault: &SkarbiecVault, item: &str) -> Value {
    serde_json::from_str(
        vault.document_of(item)["fields"]["value"]
            .as_str()
            .expect("a stored credential"),
    )
    .expect("JSON")
}

#[test]
fn held_lists_every_harness_and_account_without_the_grants() {
    let directory = TestDirectory::new("harness-held");
    let vault = SkarbiecVault::create("harness-held");
    let home = home_with_every_harness(&directory, &[PRIMARY, SECOND]);
    let (status, stdout, stderr) = brama(
        &vault,
        &[
            "subscription",
            "held",
            "--home",
            home.to_str().unwrap(),
            "--json",
        ],
        None,
    );
    assert_eq!(status, 0, "{stderr}");
    let views: Vec<Value> = serde_json::from_str(&stdout).expect("a JSON list");
    let rows: Vec<(String, String, String)> = views
        .iter()
        .map(|view| {
            (
                view["harness"].as_str().unwrap().into(),
                view["provider"].as_str().unwrap().into(),
                view["account"].as_str().unwrap_or("").into(),
            )
        })
        .collect();
    assert!(
        rows.contains(&("omp".into(), "claude-code".into(), PRIMARY.into())),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("omp".into(), "claude-code".into(), SECOND.into())),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("omp".into(), "codex".into(), "lukasz@wisent.com".into())),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("codex".into(), "codex".into(), "codex@wisent.ai".into())),
        "the account is read from the identity token: {rows:?}"
    );
    assert!(
        rows.contains(&("kimi".into(), "kimi".into(), "".into())),
        "{rows:?}"
    );
    assert!(
        rows.contains(&("claude".into(), "claude-code".into(), "".into())),
        "{rows:?}"
    );
    assert!(
        !rows.iter().any(|row| row.2 == LAPSED),
        "a disabled account is not held"
    );
    assert!(
        !stdout.contains("sk-ort-") && !stdout.contains("sk-oat-"),
        "no grant is listed"
    );
}

#[test]
fn a_grant_from_each_harness_is_stored_in_its_providers_shape() {
    let directory = TestDirectory::new("harness-each");
    let vault = SkarbiecVault::create("harness-each");
    let item = vault.seed_subscription(AGENT, "codex", "pool-agent");
    let home = home_with_every_harness(&directory, &[PRIMARY]);
    let (_, stdout, stderr) = import(&vault, &home, "codex", &["--from", "codex"]);
    assert!(
        stdout.contains("account: codex@wisent.ai") && stdout.contains("is stored, but"),
        "{stdout}{stderr}"
    );
    let document = stored(&vault, &item);
    assert_eq!(document["tokens"]["refresh_token"], "sk-ort-codex-cli");
    assert_eq!(
        document["tokens"]["account_id"], "acct-codex-cli",
        "the account id the ChatGPT backend is told"
    );
    let (_, stdout, stderr) = import(&vault, &home, "codex", &["--from", "omp"]);
    assert!(
        stdout.contains("account: lukasz@wisent.com"),
        "{stdout}{stderr}"
    );
    let document = stored(&vault, &item);
    assert_eq!(
        document["tokens"]["refresh_token"],
        "sk-ort-lukasz@wisent.com"
    );
    assert_eq!(document["tokens"]["account_id"], "acct-lukasz@wisent.com");
    assert_eq!(document["auth_mode"], "chatgpt");
    let kimi = vault.seed_subscription(AGENT, "kimi", "pool-agent");
    let (_, stdout, stderr) = import(&vault, &home, "kimi", &[]);
    assert!(
        stdout.contains("is stored, but"),
        "the only kimi grant needs no --from: {stdout}{stderr}"
    );
    let document = stored(&vault, &kimi);
    assert_eq!(document["refresh_token"], "sk-ort-kimi");
    assert_eq!(
        document["expires_at"], 1_789_400_000_i64,
        "kimi's expiry stays in seconds"
    );
    let claude = vault.seed_subscription(AGENT, "claude-code", "pool-agent");
    let (_, stdout, stderr) = import(&vault, &home, "claude-code", &["--from", "claude"]);
    assert!(stdout.contains("is stored, but"), "{stdout}{stderr}");
    assert_eq!(
        stored(&vault, &claude)["claudeAiOauth"]["refreshToken"],
        "sk-ort-claude-code"
    );
    let (_, stdout, stderr) = import(
        &vault,
        &home,
        "claude-code",
        &["--from", "omp", "--account", PRIMARY],
    );
    assert!(
        stdout.contains(&format!("account: {PRIMARY}")),
        "{stdout}{stderr}"
    );
    let document = stored(&vault, &claude);
    assert_eq!(
        document["claudeAiOauth"]["refreshToken"],
        format!("sk-ort-{PRIMARY}")
    );
    assert_eq!(
        document["claudeAiOauth"]["expiresAt"],
        1_789_400_000_000_i64
    );
    assert!(document["claudeAiOauth"]["scopes"]
        .as_array()
        .is_some_and(|scopes| !scopes.is_empty()));
}

#[test]
fn two_grants_for_one_provider_are_refused_until_one_is_named() {
    let directory = TestDirectory::new("harness-two");
    let vault = SkarbiecVault::create("harness-two");
    let item = vault.seed_subscription(AGENT, "codex", "pool-agent");
    let home = home_with_every_harness(&directory, &[PRIMARY]);
    let (status, _, stderr) = import(&vault, &home, "codex", &[]);
    assert_eq!(status, 1);
    assert!(
        stderr.contains("2 grants for `codex` are held here"),
        "{stderr}"
    );
    assert!(
        stderr.contains("omp (lukasz@wisent.com)") && stderr.contains("codex (codex@wisent.ai)"),
        "{stderr}"
    );
    assert_eq!(
        vault.document_of(&item)["fields"]["value"],
        "seeded-pool-agent",
        "nothing was stored"
    );
    let (status, _, stderr) = import(&vault, &home, "codex", &["--from", "weles"]);
    assert_eq!(status, 1);
    assert!(
        stderr.contains("`weles` is not a harness Brama reads grants from"),
        "{stderr}"
    );
    let (status, _, stderr) = import(&vault, &home, "kimi", &["--from", "codex"]);
    assert_eq!(status, 1);
    assert!(
        stderr.contains("codex signs nothing in for `kimi`"),
        "{stderr}"
    );
    let (status, _, stderr) = import(&vault, &directory.path().join("nowhere"), "kimi", &[]);
    assert_eq!(status, 1);
    assert!(
        stderr.contains("no harness under") && stderr.contains("holds an enabled `kimi` grant"),
        "{stderr}"
    );
}

#[test]
fn a_document_without_a_refresh_token_is_refused_before_anything_is_stored() {
    let gateway = Gateway::start("grant-unrenewable", &[(AGENT, "codex", "pool-agent")]);
    let (status, answer) = gateway.console(
        GRANT,
        Method::POST,
        Some(
            &json!({"subscription_id": "pool-agent", "reason": "story", "harness": "codex",
                     "document": json!({"tokens": {"access_token": "sk-oat-only"}}).to_string()}),
        ),
    );
    assert_eq!(status, 409, "{answer}");
    let (_, _, message) = refusal(&answer);
    assert!(message.contains("carries no refresh token"), "{message}");
    let (status, answer) = gateway.console(
        GRANT,
        Method::POST,
        Some(
            &json!({"subscription_id": "pool-agent", "reason": "story", "harness": "weles",
                     "document": json!({"tokens": {"refresh_token": "sk-ort"}}).to_string()}),
        ),
    );
    assert_eq!(status, 400, "{answer}");
}

/// The desktop's path end to end: the binary beside the harness reads the
/// grant and hands it to a gateway elsewhere with the console's bearer on
/// stdin; the gateway stores it, the provider refuses the made-up token, and
/// a row the ledger had disowned reads active again.
#[test]
fn a_grant_handed_through_a_gateway_gets_the_providers_verdict_and_repairs_the_row() {
    let directory = TestDirectory::new("grant-through-gateway");
    let gateway = Gateway::start("grant-through-gateway", &[(AGENT, "codex", "pool-agent")]);
    let home = home_with_every_harness(&directory, &[PRIMARY]);
    let (status, refreshed) = gateway.console(
        "/v1/admin/subscription-pool/refresh",
        Method::POST,
        Some(&json!({"provider": "codex", "reason": "story"})),
    );
    assert_eq!(status, 200, "{refreshed}");
    let state = |report: &Value| {
        report["subscriptions"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"] == "pool-agent"))
            .map(|row| row["credential"]["state"].clone())
            .unwrap_or(Value::Null)
    };
    let (status, stdout, stderr) = brama(
        gateway.vault(),
        &[
            "subscription",
            "import",
            "codex",
            "--from",
            "codex",
            "--subscription-id",
            "pool-agent",
            "--reason",
            "story",
            "--home",
            home.to_str().unwrap(),
            "--gateway",
            gateway.origin(),
            "--json",
        ],
        Some(CONSOLE_BEARER),
    );
    assert_eq!(
        status, 1,
        "a made-up grant is refused by the provider:\n{stdout}{stderr}"
    );
    let verdict: Value = serde_json::from_str(&stdout).expect("a JSON verdict");
    assert_eq!(verdict["account"], "codex@wisent.ai");
    assert_eq!(verdict["result"], "failed");
    assert!(
        verdict["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("codex, the harness that held it")
                && detail.contains("would not serve it")),
        "the provider, not Brama, refused it: {verdict}"
    );
    let (_, after) = gateway.console(gateway::POOL, Method::GET, None);
    assert_eq!(
        state(&after),
        "active",
        "the handed-over grant is the sign-in the ledger waited for: {after}"
    );
    let (status, _, stderr) = brama(
        gateway.vault(),
        &[
            "subscription",
            "import",
            "codex",
            "--from",
            "codex",
            "--subscription-id",
            "pool-agent",
            "--reason",
            "story",
            "--home",
            home.to_str().unwrap(),
            "--gateway",
            gateway.origin(),
        ],
        Some(""),
    );
    assert_eq!(status, 1);
    assert!(stderr.contains("stdin was empty"), "{stderr}");
}
