//! One story per provider-facing command, across every provider in the
//! descriptor table.
//!
//! The table in `src/providers/adapter.rs` splits the 23 providers into three
//! credential families -- three OAuth subscription providers, nineteen API-key
//! providers, and the routes-file `local-openai` -- and the commands below
//! answer differently per family. Every sentence asserted here was copied from
//! a live answer of the built binary against a seeded state, never guessed.
//!
//! Nothing here contacts a provider, opens a browser, or spends quota: each
//! path exercised is a refusal or a read, which is exactly the part of the
//! contract that must hold on a machine with no credentials at all.

#[path = "../support/mod.rs"]
mod support;

use std::process::Command;

use serde_json::Value;
use support::TestDirectory;

/// Every provider id in the descriptor table, in declaration order.
const ALL_PROVIDERS: &[&str] = &[
    "anthropic",
    "claude-code",
    "kimi",
    "openai",
    "codex",
    "openrouter",
    "groq",
    "mistral",
    "xai",
    "deepseek",
    "cerebras",
    "fireworks",
    "together",
    "nvidia",
    "moonshot",
    "zai",
    "qwen",
    "huggingface",
    "featherless",
    "venice",
    "novita",
    "synthetic",
    "local-openai",
];

/// The providers whose subscription credentials are OAuth grants Brama can
/// refresh and Weles can sign in.
const OAUTH_PROVIDERS: &[&str] = &["claude-code", "codex", "kimi"];

fn is_oauth(provider: &str) -> bool {
    OAUTH_PROVIDERS.contains(&provider)
}

fn command(directory: &TestDirectory) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    command
        .env_remove("WELES_API_TOKEN")
        .env_remove("WELES_WORKER_ENV_FILE")
        .env_remove("BRAMA_SUBSCRIPTION_CATALOG")
        .env("HOME", directory.path().join("home"))
        .env("XDG_STATE_HOME", directory.path().join("xdg-state"))
        .env("BRAMA_STATE_DIR", directory.path().join("state"))
        .env(
            "BRAMA_SUBSCRIPTION_USAGE_FILE",
            directory.path().join("usage.json"),
        )
        .env(
            "ENTITLEMENTS_ROUTER_BIN",
            directory.path().join("absent-router"),
        );
    command
}

/// A usage ledger holding exactly one never-touched subscription per provider,
/// id `probe-<provider>`, in the shape `subscription_dispatch::usage` persists.
fn seed_ledger(directory: &TestDirectory, providers: &[&str]) {
    let rows: Vec<String> = providers
        .iter()
        .map(|provider| format!(r#""probe-{provider}":{{"provider":"{provider}"}}"#))
        .collect();
    std::fs::write(
        directory.path().join("usage.json"),
        format!(r#"{{"subscriptions":{{{}}}}}"#, rows.join(",")),
    )
    .expect("seed usage ledger");
}

fn stdout_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The journal records the state directory holds, newest last.
fn journal_records(directory: &TestDirectory) -> Vec<Value> {
    let path = directory.path().join("state").join("journal.jsonl");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("journal line is JSON"))
        .collect()
}

#[test]
fn refresh_names_the_empty_pool_for_every_provider() {
    let directory = TestDirectory::new("providers-refresh-empty");
    for provider in ALL_PROVIDERS {
        let output = command(&directory)
            .args([
                "subscription",
                "refresh",
                provider,
                "--reason",
                "provider contract: empty pool",
            ])
            .output()
            .expect("brama subscription refresh");
        assert_eq!(
            output.status.code(),
            Some(1),
            "an empty pool must exit non-zero for {provider}"
        );
        let expected = format!(
            "no usable `{provider}` subscription is in this deployment's pool, so no credential \
             source is configured to refresh: one has to be signed in and stored in the vault \
             before this command has anything to act on"
        );
        assert!(
            stdout_of(&output).contains(&expected),
            "empty-pool sentence missing for {provider}: {}",
            stdout_of(&output)
        );
    }
    // Every attempt is journaled with its reason, verbatim, including the ones
    // that found nothing to do.
    let records = journal_records(&directory);
    assert_eq!(records.len(), ALL_PROVIDERS.len());
    for (record, provider) in records.iter().zip(ALL_PROVIDERS) {
        assert_eq!(record["kind"], "subscription_refresh");
        assert_eq!(record["provider"], *provider);
        assert_eq!(record["reason"], "provider contract: empty pool");
        assert_eq!(record["result"], "failed");
        assert_eq!(record["attempted"], 0);
    }
}

#[test]
fn refresh_refuses_api_key_providers_with_the_no_oauth_sentence() {
    let directory = TestDirectory::new("providers-refresh-api-key");
    let api_key_providers: Vec<&&str> = ALL_PROVIDERS
        .iter()
        .filter(|provider| !is_oauth(provider))
        .collect();
    for provider in &api_key_providers {
        seed_ledger(&directory, &[provider]);
        let output = command(&directory)
            .args([
                "subscription",
                "refresh",
                provider,
                "--reason",
                "provider contract: api key has no refresh path",
            ])
            .output()
            .expect("brama subscription refresh");
        assert_eq!(output.status.code(), Some(1), "{provider} must exit 1");
        let expected = format!(
            "`{provider}` subscription credentials are API keys rather than OAuth grants, so no \
             refresh path exists for them: replacing one means storing a new credential in the \
             vault"
        );
        assert!(
            stdout_of(&output).contains(&expected),
            "API-key sentence missing for {provider}: {}",
            stdout_of(&output)
        );
    }
}

#[test]
fn refresh_attempts_oauth_providers_and_reports_the_redeem_refusal() {
    let directory = TestDirectory::new("providers-refresh-oauth");
    for provider in OAUTH_PROVIDERS {
        seed_ledger(&directory, &[provider]);
        let output = command(&directory)
            .args([
                "subscription",
                "refresh",
                provider,
                "--reason",
                "provider contract: redeem refusal",
            ])
            .output()
            .expect("brama subscription refresh");
        assert_eq!(output.status.code(), Some(1), "{provider} must exit 1");
        let stdout = stdout_of(&output);
        // One candidate was found and tried; the refusal never reached the
        // provider because nothing in this environment can produce a credential.
        assert!(stdout.contains("attempted: 1"), "{provider}: {stdout}");
        assert!(
            stdout.contains(&format!("refreshed no `{provider}` grant out of 1 tried")),
            "{provider}: {stdout}"
        );
        assert!(
            stdout.contains(&format!(
                "probe-{provider}: no capability or read grant produced this subscription's \
                 credential"
            )),
            "{provider}: {stdout}"
        );
    }
}

#[test]
fn sign_in_refuses_every_provider_weles_cannot_sign_in() {
    let directory = TestDirectory::new("providers-sign-in-unknown");
    for provider in ALL_PROVIDERS.iter().filter(|provider| !is_oauth(provider)) {
        let output = command(&directory)
            .args([
                "subscription",
                "sign-in",
                provider,
                "--reason",
                "provider contract: not a subscription provider",
            ])
            .output()
            .expect("brama subscription sign-in");
        assert_eq!(output.status.code(), Some(1), "{provider} must exit 1");
        let expected =
            format!("Weles signs in claude-code, codex and kimi; `{provider}` is not one of them");
        assert!(
            stderr_of(&output).contains(&expected),
            "unknown-provider sentence missing for {provider}: {}",
            stderr_of(&output)
        );
    }
    // A hard refusal reaches no verdict, so nothing may be journaled for it.
    assert!(
        journal_records(&directory).is_empty(),
        "a refused sign-in must not journal a verdict"
    );
}

#[test]
fn sign_in_refuses_oauth_providers_without_a_weles_worker() {
    let directory = TestDirectory::new("providers-sign-in-no-worker");
    for provider in OAUTH_PROVIDERS {
        // An isolated command receives no Brama-Weles credential. The refusal
        // names the exact Skarbiec item the service launcher must acquire.
        let output = command(&directory)
            .args([
                "subscription",
                "sign-in",
                provider,
                "--reason",
                "provider contract: no worker on this host",
            ])
            .output()
            .expect("brama subscription sign-in");
        assert_eq!(output.status.code(), Some(1), "{provider} must exit 1");
        assert!(
            stderr_of(&output).contains(
                "BRAMA_WELES_REAUTH_TOKEN is unavailable; Brama must acquire \
                 brama-weles-reauth/token from Skarbiec at startup"
            ),
            "{provider}: {}",
            stderr_of(&output)
        );

        // With Brama's route credential but no worker listening, the refusal
        // names the exact health endpoint before any sign-in is attempted.
        let output = command(&directory)
            .env("BRAMA_WELES_REAUTH_TOKEN", "provider-contract-token")
            .env("BRAMA_WELES_URL", "http://127.0.0.1:1")
            .args([
                "subscription",
                "sign-in",
                provider,
                "--reason",
                "provider contract: worker unreachable",
            ])
            .output()
            .expect("brama subscription sign-in");
        assert_eq!(output.status.code(), Some(1), "{provider} must exit 1");
        assert!(
            stderr_of(&output).contains(
                "Weles worker API does not answer its own health check at \
                 http://127.0.0.1:1/healthz"
            ),
            "{provider}: {}",
            stderr_of(&output)
        );
        assert!(
            stderr_of(&output).contains("start it before signing an account in"),
            "{provider}: {}",
            stderr_of(&output)
        );
    }
    assert!(
        journal_records(&directory).is_empty(),
        "a refused sign-in must not journal a verdict"
    );
}

#[test]
fn subscriptions_list_reports_one_row_per_seeded_provider() {
    let directory = TestDirectory::new("providers-list");
    seed_ledger(&directory, ALL_PROVIDERS);
    let before = std::fs::read(directory.path().join("usage.json")).expect("seeded ledger");

    let output = command(&directory)
        .args(["subscriptions", "list"])
        .output()
        .expect("brama subscriptions list");
    assert!(output.status.success(), "{}", stderr_of(&output));
    let stdout = stdout_of(&output);
    assert!(
        stdout.contains(&format!(
            "0 of {} subscription credentials are live",
            ALL_PROVIDERS.len()
        )),
        "count line missing: {stdout}"
    );
    for provider in ALL_PROVIDERS {
        // A subscription whose grant nothing has ever looked at is `unknown`,
        // which is deliberately not the same statement as a working one.
        assert!(
            stdout.contains(&format!("unknown  {provider}")),
            "row missing for {provider}: {stdout}"
        );
        assert!(stdout.contains(&format!("probe-{provider}")));
    }

    let json = command(&directory)
        .args(["subscriptions", "list", "--json"])
        .output()
        .expect("brama subscriptions list --json");
    assert!(json.status.success());
    let report: Value = serde_json::from_slice(&json.stdout).expect("report is JSON");
    let rows = report["providers"].as_array().expect("providers array");
    assert_eq!(rows.len(), ALL_PROVIDERS.len());
    for row in rows {
        let provider = row["provider"].as_str().expect("provider");
        assert!(ALL_PROVIDERS.contains(&provider), "unexpected {provider}");
        assert_eq!(row["state"], "unknown");
        assert_eq!(
            row["subscription_id"],
            Value::String(format!("probe-{provider}"))
        );
        assert_eq!(row["expires_at"], Value::Null);
        assert_eq!(row["last_redeem_error"], Value::Null);
    }

    // Listing is read-only: the ledger file must hold exactly the bytes the
    // seed wrote, and no journal record may exist.
    let after = std::fs::read(directory.path().join("usage.json")).expect("ledger after list");
    assert_eq!(before, after, "a listing must not rewrite the ledger");
    assert!(journal_records(&directory).is_empty());
}

#[test]
fn test_command_refuses_billable_inference_for_every_provider_route() {
    let directory = TestDirectory::new("providers-test-refusal");
    for provider in ALL_PROVIDERS {
        let output = command(&directory)
            .args(["test", "--model", &format!("{provider}/any-model")])
            .output()
            .expect("brama test");
        assert_eq!(output.status.code(), Some(1), "{provider} must exit 1");
        assert!(
            stderr_of(&output)
                .contains("refusing billable inference without explicit --allow-provider-cost"),
            "{provider}: {}",
            stderr_of(&output)
        );
    }
}
