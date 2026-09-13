//! Taking the Claude grant the operator's harness already holds.
//!
//! The harness keeps its credentials in `~/.omp/agent/agent.db`, table
//! `auth_credentials`, one row per account, the grant as a JSON document.
//! These stories build that store with the schema the harness creates, in
//! an isolated directory, and drive the real binary against it and a real
//! isolated vault. The grants are not real, so the provider refuses the
//! proof; what the stories assert is what Brama did before that answer -
//! which account it took, what it stored and in what shape - and how it
//! refuses when the store cannot answer.

#[path = "../support/gateway.rs"]
mod gateway;
#[path = "../support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::Command;

use reqwest::Method;
use rusqlite::Connection;
use serde_json::{json, Value};

use gateway::{refusal, Gateway, AGENT};
use support::{SkarbiecVault, TestDirectory};

const PRIMARY: &str = "lbartoszcze@wisent.ai";
const SECOND: &str = "controlyourai@gmail.com";
const LAPSED: &str = "lukasz.bartoszcze@wisent.ai";

/// The harness's store, created the way the harness creates it.
fn harness_store(directory: &TestDirectory, enabled: &[&str]) -> PathBuf {
    let path = directory.path().join("agent.db");
    let store = Connection::open(&path).expect("create the harness store");
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
        .expect("the harness schema");
    let mut insert = store
        .prepare(
            "INSERT INTO auth_credentials (provider, credential_type, data, disabled_cause, identity_key, updated_at)
             VALUES (?1, 'oauth', ?2, ?3, ?4, ?5)",
        )
        .expect("prepare");
    let mut row = |email: &str, disabled: Option<&str>, updated_at: i64| {
        let data = json!({
            "access": format!("sk-ant-oat01-{email}"),
            "refresh": format!("sk-ant-ort01-{email}"),
            "expires": 1_789_400_000_000_i64,
            "accountId": "a4b15636-8b05-4815-960f-09ec94477042",
            "email": email,
            "orgId": "15cf4e03-a1b3-4e3b-8d57-d48d634ded27",
            "orgName": "Wisent",
            "authorizedAt": 1_789_300_000_000_i64,
        })
        .to_string();
        insert
            .execute(rusqlite::params![
                "anthropic",
                data,
                disabled,
                format!("email:{email}"),
                updated_at
            ])
            .expect("insert a harness credential");
    };
    for (index, email) in enabled.iter().enumerate() {
        row(email, None, 1_789_300_000 + index as i64);
    }
    row(LAPSED, Some("deleted by user"), 1_789_300_999);
    insert
        .execute(rusqlite::params![
            "openai-codex",
            json!({"email": "lukasz@wisent.com"}).to_string(),
            Option::<&str>::None,
            "email:lukasz@wisent.com",
            1_789_300_500
        ])
        .expect("insert a codex credential");
    path
}

/// `brama subscription import` over the isolated vault, as an operator runs it.
fn import(vault: &SkarbiecVault, store: &Path, extra: &[&str]) -> (i32, String, String) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    command
        .args(["subscription", "import", "claude-code", "--from-omp"])
        .args([
            "--subscription-id",
            "pool-agent-claude",
            "--reason",
            "story",
        ])
        .args(["--store", store.to_str().expect("utf-8 path")])
        .args(extra)
        .env("ENTITLEMENTS_ROUTER_BIN", vault.router());
    for (name, value) in vault.environment() {
        command.env(name, value);
    }
    let output = command.output().expect("run the real brama binary");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn the_only_enabled_grant_is_taken_and_stored_in_the_shape_the_refresh_path_reads() {
    let directory = TestDirectory::new("import-from-omp-one");
    let vault = SkarbiecVault::create("import-from-omp-one");
    let item = vault.seed_subscription(AGENT, "claude-code", "pool-agent-claude");
    let store = harness_store(&directory, &[PRIMARY]);
    let (status, stdout, stderr) = import(&vault, &store, &[]);
    assert!(
        stdout.contains(&format!("account: {PRIMARY}")),
        "{stdout}{stderr}"
    );
    assert!(
        stdout.contains("result: failed"),
        "a made-up grant is refused by the provider:\n{stdout}{stderr}"
    );
    assert!(
        stdout.contains("is stored, but"),
        "the grant was stored before the proof:\n{stdout}{stderr}"
    );
    assert_eq!(status, 1);
    let document = vault.document_of(&item);
    let grant = &document["fields"]["value"];
    let stored: Value =
        serde_json::from_str(grant.as_str().expect("the credential is a JSON string"))
            .expect("JSON");
    assert_eq!(
        stored["claudeAiOauth"]["accessToken"],
        format!("sk-ant-oat01-{PRIMARY}")
    );
    assert_eq!(
        stored["claudeAiOauth"]["refreshToken"],
        format!("sk-ant-ort01-{PRIMARY}")
    );
    assert_eq!(stored["claudeAiOauth"]["expiresAt"], 1_789_400_000_000_i64);
    assert!(stored["claudeAiOauth"]["scopes"]
        .as_array()
        .is_some_and(|scopes| !scopes.is_empty()));
    assert!(
        vault
            .tags_of(&item)
            .iter()
            .any(|tag| tag == "brama:subscription"),
        "the item keeps the tags discovery reads"
    );
}

#[test]
fn two_enabled_accounts_are_refused_until_one_is_named() {
    let directory = TestDirectory::new("import-from-omp-two");
    let vault = SkarbiecVault::create("import-from-omp-two");
    let item = vault.seed_subscription(AGENT, "claude-code", "pool-agent-claude");
    let store = harness_store(&directory, &[PRIMARY, SECOND]);
    let (status, _, stderr) = import(&vault, &store, &[]);
    assert_eq!(status, 1);
    assert!(stderr.contains("holds 2 enabled"), "{stderr}");
    assert!(
        stderr.contains(PRIMARY) && stderr.contains(SECOND),
        "both choices are named:\n{stderr}"
    );
    assert!(
        !stderr.contains(LAPSED),
        "a disabled account is not a choice"
    );
    assert_eq!(
        vault.document_of(&item)["fields"]["value"],
        "seeded-pool-agent-claude",
        "nothing was stored"
    );
    let (_, stdout, stderr) = import(&vault, &store, &["--account", SECOND]);
    assert!(
        stdout.contains(&format!("account: {SECOND}")),
        "{stdout}{stderr}"
    );
    let stored: Value = serde_json::from_str(
        vault.document_of(&item)["fields"]["value"]
            .as_str()
            .expect("stored"),
    )
    .expect("JSON");
    assert_eq!(
        stored["claudeAiOauth"]["refreshToken"],
        format!("sk-ant-ort01-{SECOND}")
    );
}

#[test]
fn a_store_without_a_grant_and_a_missing_store_are_refused_by_name() {
    let directory = TestDirectory::new("import-from-omp-none");
    let vault = SkarbiecVault::create("import-from-omp-none");
    vault.seed_subscription(AGENT, "claude-code", "pool-agent-claude");
    let store = harness_store(&directory, &[]);
    let (status, _, stderr) = import(&vault, &store, &[]);
    assert_eq!(status, 1);
    assert!(
        stderr.contains("holds no enabled `claude-code` grant"),
        "{stderr}"
    );
    let (status, _, stderr) = import(&vault, &directory.path().join("nowhere.db"), &[]);
    assert_eq!(status, 1);
    assert!(stderr.contains("cannot be opened"), "{stderr}");
    let (status, _, stderr) = import(&vault, &store, &["--account", "nobody@wisent.ai"]);
    assert_eq!(status, 1);
    assert!(
        stderr.contains("no enabled `claude-code` grant for nobody@wisent.ai"),
        "{stderr}"
    );
}

#[test]
fn the_console_hands_a_grant_over_and_gets_the_same_verdict() {
    let gateway = Gateway::start(
        "grant-from-console",
        &[(AGENT, "claude-code", "pool-agent-claude")],
    );
    let body = json!({
        "subscription_id": "pool-agent-claude",
        "reason": "story",
        "access_token": "sk-ant-oat01-console",
        "refresh_token": "sk-ant-ort01-console",
        "expires_at_ms": 1_789_400_000_000_i64,
        "account": PRIMARY,
    });
    let (status, answer) = gateway.console(
        "/v1/admin/subscription-pool/grant",
        Method::POST,
        Some(&body),
    );
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["provider"], "claude-code");
    assert_eq!(answer["subscription_id"], "pool-agent-claude");
    assert_eq!(answer["account"], PRIMARY);
    assert_eq!(
        answer["result"], "failed",
        "a made-up grant is refused by the provider: {answer}"
    );
    assert!(
        answer["detail"].as_str().is_some_and(|detail| detail.contains("is stored, but the provider would not serve it")),
        "the provider, not Brama, refused the made-up token: {answer}"
    );
    let (status, answer) = gateway.agent(
        "/v1/admin/subscription-pool/grant",
        Method::POST,
        Some(&body),
    );
    assert_eq!(status, 403, "{answer}");
    let mut blank = body.clone();
    blank["reason"] = json!(" ");
    let (status, answer) = gateway.console(
        "/v1/admin/subscription-pool/grant",
        Method::POST,
        Some(&blank),
    );
    assert_eq!(status, 400, "{answer}");
    let (_, _, message) = refusal(&answer);
    assert!(message.contains("--reason must say why"), "{message}");
}
