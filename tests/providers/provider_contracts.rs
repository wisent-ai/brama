//! One story per provider-facing command, across every provider in the
//! descriptor table.
//!
//! The table in `src/providers/adapter.rs` splits the 23 providers into three
//! credential families -- three OAuth subscription providers, nineteen
//! API-key providers, and the routes-file `local-openai` -- and each command
//! answers differently per family. Every sentence asserted here was copied
//! from a live answer of the built binary, never guessed.
//!
//! The inventory behind these commands is a real Skarbiec vault this test
//! created with `skarbiec init` and seeded with real `skarbiec set-json`
//! writes. It used to be a three-line `/bin/sh` script answering `list`, so
//! every story here rested on a stand-in for the product Brama shells out to.
//! Nothing here contacts a provider, opens a browser, or spends quota.
//!
//! The seeded read of the same pool lives beside its unreadable counterpart,
//! in `tests/cli/command_contracts.rs`.

#[path = "../support/mod.rs"]
mod support;

use std::process::{Command, Output};

use serde_json::Value;
use support::{SkarbiecVault, TestDirectory};

/// Every provider id in the descriptor table, in declaration order.
#[rustfmt::skip]
const ALL_PROVIDERS: &[&str] = &[
    "anthropic", "claude-code", "kimi", "openai", "codex", "openrouter",
    "groq", "mistral", "xai", "deepseek", "cerebras", "fireworks", "together",
    "nvidia", "moonshot", "zai", "qwen", "huggingface", "featherless",
    "venice", "novita", "synthetic", "local-openai",
];

/// The providers whose subscription credentials are OAuth grants Brama can
/// refresh and Weles can sign in.
const OAUTH_PROVIDERS: &[&str] = &["claude-code", "codex", "kimi"];

/// The agent the vault names as owner of every seeded account.
const AGENT: &str = "brama-provider-contracts";
/// A worker address nothing listens on, so a sign-in story is about the
/// credential it says it is about rather than about a Weles that answered.
const UNREACHABLE_WORKER: &str = "http://127.0.0.1:1";

fn is_oauth(provider: &str) -> bool {
    OAUTH_PROVIDERS.contains(&provider)
}

/// One command against one real vault and one test-owned state area. The
/// vault's environment carries HOME, GNUPGHOME and the vault path, and Brama
/// hands its whole environment to the real router child.
fn run(
    directory: &TestDirectory,
    vault: &SkarbiecVault,
    environment: &[(&str, &str)],
    args: &[&str],
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    command
        .env_remove("WELES_API_TOKEN")
        .env_remove("WELES_WORKER_ENV_FILE")
        .env_remove("BRAMA_SUBSCRIPTION_CATALOG")
        .env_remove("BRAMA_STADO_BIN")
        .env_remove("BRAMA_WELES_URL")
        .env_remove("BRAMA_WELES_REAUTH_TOKEN");
    for (name, value) in vault.environment() {
        command.env(name, value);
    }
    command
        .env("XDG_STATE_HOME", directory.path().join("xdg-state"))
        .env("BRAMA_STATE_DIR", directory.path().join("state"))
        .env(
            "BRAMA_SUBSCRIPTION_USAGE_FILE",
            directory.path().join("usage.json"),
        )
        .env("ENTITLEMENTS_ROUTER_BIN", vault.router())
        .envs(environment.iter().copied())
        .args(args)
        .output()
        .expect("run the real brama binary")
}

/// One never-touched subscription per provider, id `probe-<provider>`, in both
/// places a subscription has to exist to be one: the vault that declares it
/// and the usage ledger that records what is known about it.
fn seed_ledger(directory: &TestDirectory, vault: &SkarbiecVault, providers: &[&str]) {
    let rows: Vec<String> = providers
        .iter()
        .map(|provider| format!(r#""probe-{provider}":{{"provider":"{provider}"}}"#))
        .collect();
    std::fs::write(
        directory.path().join("usage.json"),
        format!(r#"{{"subscriptions":{{{}}}}}"#, rows.join(",")),
    )
    .expect("seed usage ledger");
    for provider in providers {
        vault.seed_subscription(AGENT, provider, &format!("probe-{provider}"));
    }
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
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

/// `subscription refresh` against a vault that answers and holds nothing, and
/// journals every attempt with its reason verbatim -- including the ones that
/// found nothing to do.
#[test]
fn refresh_names_the_empty_pool_for_every_provider() {
    let directory = TestDirectory::new("providers-refresh-empty");
    let vault = SkarbiecVault::create("prov-refresh-empty");
    let reason = "provider contract: empty pool";
    for provider in ALL_PROVIDERS {
        let output = run(
            &directory,
            &vault,
            &[],
            &["subscription", "refresh", provider, "--reason", reason],
        );
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
    let records = journal_records(&directory);
    assert_eq!(records.len(), ALL_PROVIDERS.len());
    for (record, provider) in records.iter().zip(ALL_PROVIDERS) {
        assert_eq!(record["kind"], "subscription_refresh");
        assert_eq!(record["provider"], *provider);
        assert_eq!(record["reason"], reason);
        assert_eq!(record["result"], "failed");
        assert_eq!(record["attempted"], u32::MIN);
    }
}

/// `subscription refresh` over a vault holding one account per provider: an
/// API-key provider has no refresh path at all, and an OAuth provider tries
/// the account it found and reports why no grant came of it.
#[test]
fn refresh_answers_each_credential_family_in_its_own_words() {
    let directory = TestDirectory::new("providers-refresh-family");
    let vault = SkarbiecVault::create("prov-refresh-family");
    let reason = "provider contract: credential family";
    for provider in ALL_PROVIDERS {
        seed_ledger(&directory, &vault, &[provider]);
        let output = run(
            &directory,
            &vault,
            &[],
            &["subscription", "refresh", provider, "--reason", reason],
        );
        assert_eq!(output.status.code(), Some(1), "{provider} must exit 1");
        let stdout = stdout_of(&output);
        if is_oauth(provider) {
            assert!(
                stdout.contains(&format!("refreshed no `{provider}` grant out of 1 tried")),
                "{provider}: {stdout}"
            );
            assert!(
                stdout.contains(&format!(
                    "no usable credential source is configured for `{provider}` in this \
                     environment"
                )),
                "{provider}: {stdout}"
            );
        } else {
            assert!(
                stdout.contains(&format!(
                    "`{provider}` subscription credentials are API keys rather than OAuth \
                     grants, so no refresh path exists for them: replacing one means storing a \
                     new credential in the vault"
                )),
                "{provider}: {stdout}"
            );
        }
    }
}

/// `subscription sign-in` refuses before Weles is reached, naming which of the
/// three reasons it refused for. A hard refusal reaches no verdict, so nothing
/// may be journaled for any of them.
#[test]
fn sign_in_refuses_before_it_reaches_weles() {
    let directory = TestDirectory::new("providers-sign-in");
    let vault = SkarbiecVault::create("prov-signin");
    for provider in ALL_PROVIDERS.iter().filter(|provider| !is_oauth(provider)) {
        let output = run(
            &directory,
            &vault,
            &[],
            &[
                "subscription",
                "sign-in",
                provider,
                "--reason",
                "provider contract: not a subscription provider",
            ],
        );
        assert_eq!(output.status.code(), Some(1), "{provider} must exit 1");
        let expected =
            format!("Weles signs in claude-code, codex and kimi; `{provider}` is not one of them");
        assert!(
            stderr_of(&output).contains(&expected),
            "unknown-provider sentence missing for {provider}: {}",
            stderr_of(&output)
        );
    }

    for provider in OAUTH_PROVIDERS {
        let arguments = [
            "subscription",
            "sign-in",
            provider,
            "--reason",
            "provider contract: no worker on this host",
        ];
        // No Brama-Weles credential: the refusal names the exact Skarbiec item
        // the launcher must acquire.
        let output = run(
            &directory,
            &vault,
            &[("BRAMA_WELES_URL", UNREACHABLE_WORKER)],
            &arguments,
        );
        assert_eq!(output.status.code(), Some(1), "{provider} must exit 1");
        assert!(
            stderr_of(&output).contains(
                "BRAMA_WELES_REAUTH_TOKEN is unavailable; Brama must acquire \
                 brama-weles-reauth/token from Skarbiec at startup"
            ),
            "{provider}: {}",
            stderr_of(&output)
        );

        // With that credential but no worker listening, the refusal names the
        // exact call that failed. A subscription id is passed because the
        // vault in this fixture holds no active row for the provider, and the
        // point here is the worker being unreachable rather than the vault
        // being empty.
        let output = run(
            &directory,
            &vault,
            &[
                ("BRAMA_WELES_REAUTH_TOKEN", "provider-contract-token"),
                ("BRAMA_WELES_URL", UNREACHABLE_WORKER),
            ],
            &[
                "subscription",
                "sign-in",
                provider,
                "--subscription-id",
                "provider-contract-subscription",
                "--reason",
                "provider contract: no worker on this host",
            ],
        );
        assert_eq!(output.status.code(), Some(1), "{provider} must exit 1");
        assert!(
            stderr_of(&output).contains(&format!("POST {UNREACHABLE_WORKER}/reauth/resolve")),
            "{provider}: {}",
            stderr_of(&output)
        );
    }
    assert!(
        journal_records(&directory).is_empty(),
        "a refused sign-in must not journal a verdict"
    );
}
