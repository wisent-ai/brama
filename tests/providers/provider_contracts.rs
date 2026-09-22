//! One story per provider-facing command, across every provider in the
//! descriptor table.
//!
//! The table in `src/providers/adapter.rs` splits the providers into three
//! credential families -- three OAuth subscription providers, the API-key
//! providers, and the routes-file `local-openai` -- and each command
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

/// Every provider id in the descriptor table, in declaration order, read
/// from the table itself: a hand-kept copy of it here drifted from the
/// roster the moment a provider was added, and these stories are about every
/// provider rather than about the ones a list remembered.
fn all_providers() -> Vec<&'static str> {
    brama::providers::adapter::providers()
        .iter()
        .map(|descriptor| descriptor.id)
        .collect()
}

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


#[path = "provider_contracts/harness.rs"]
mod harness;

use harness::{journal_records, run, seed_ledger, stderr_of, stdout_of};
/// found nothing to do.
#[test]
fn refresh_names_the_empty_pool_for_every_provider() {
    let directory = TestDirectory::new("providers-refresh-empty");
    let vault = SkarbiecVault::create("prov-refresh-empty");
    let reason = "provider contract: empty pool";
    for provider in all_providers() {
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
    assert_eq!(records.len(), all_providers().len());
    for (record, provider) in records.iter().zip(all_providers()) {
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
    for provider in all_providers() {
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
    for provider in all_providers()
        .into_iter()
        .filter(|provider| !is_oauth(provider))
    {
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
