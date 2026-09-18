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

#[path = "grants/gateway_story.rs"]
mod gateway_story;
#[path = "grants/live_story.rs"]
mod live_story;
#[path = "grants/stores.rs"]
mod stores;
#[path = "grants/sync_story.rs"]
mod sync_story;

pub(crate) use stores::*;

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
    // The account the harness recorded is written beside the grant as the
    // item's `account_ref`: what Weles resolves a sign-in from when this
    // grant dies. Until 2026-09-18 every imported member carried none and
    // `/readyz` reported each as `subscription_identity_missing`.
    assert_eq!(
        vault.document_of(&item)["context"]["account_ref"],
        "lukasz@wisent.com",
        "the imported member names its account for the sign-in that replaces it"
    );
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

/// The state the four imported members were in on 2026-09-18: the item holds
/// the very grant the harness still holds, and no account. The sweep hands
/// that grant over on every pass; comparing the grant alone answered
/// "unchanged" every time and the account never reached the item, so Weles
/// refused every automatic sign-in with `subscription_identity_missing`.
#[test]
fn an_unchanged_grant_on_an_item_without_an_account_still_names_the_account() {
    let directory = TestDirectory::new("harness-account");
    let vault = SkarbiecVault::create("harness-account");
    let item = vault.seed_subscription(AGENT, "claude-code", "pool-agent");
    let home = home_with_every_harness(&directory, &[PRIMARY]);
    let hand_over = ["--from", "omp", "--account", PRIMARY];
    let (_, stdout, stderr) = import(&vault, &home, "claude-code", &hand_over);
    assert!(
        stdout.contains(&format!("account: {PRIMARY}")),
        "{stdout}{stderr}"
    );
    let grant = stored(&vault, &item);
    assert_eq!(vault.document_of(&item)["context"]["account_ref"], PRIMARY);

    // The item as an earlier Brama left it: the same grant, no account.
    let mut without_account = vault.document_of(&item);
    without_account["context"]
        .as_object_mut()
        .expect("context object")
        .remove("account_ref");
    vault.replace_document(&item, &without_account);
    assert!(vault.document_of(&item)["context"]["account_ref"].is_null());

    // The same hand-over again: the grant is unchanged, the account is not.
    let (_, stdout, stderr) = import(&vault, &home, "claude-code", &hand_over);
    assert!(
        stdout.contains("is stored, but"),
        "an unchanged grant on an item without an account is still stored: {stdout}{stderr}"
    );
    // `last_refresh` is stamped at read time and is not part of the grant.
    let mut before = grant.clone();
    let mut after = stored(&vault, &item);
    for document in [&mut before, &mut after] {
        document
            .as_object_mut()
            .expect("grant object")
            .remove("last_refresh");
    }
    assert_eq!(after, before, "the grant itself is untouched");
    assert_eq!(
        vault.document_of(&item)["context"]["account_ref"],
        PRIMARY,
        "the item now names the account Weles signs it in from"
    );

    // And once it names it, the hand-over is unchanged.
    let (_, stdout, stderr) = import(&vault, &home, "claude-code", &hand_over);
    assert!(
        stdout.contains(&format!(
            "under {PRIMARY}'s name; nothing was stored or proved"
        )),
        "{stdout}{stderr}"
    );
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
