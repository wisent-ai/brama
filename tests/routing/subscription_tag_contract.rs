//! A subscription credential write must not be able to mint an account that
//! discovery cannot see.
//!
//! Discovery finds an account by `brama:subscription` (`parse_live_subscriptions`).
//! An item missing the mark is not a degraded account: it does not exist for
//! any caller, while its credential stays valid and every check that counts
//! credentials keeps answering green. That asymmetry is why this shape is
//! expensive.
//!
//! Until 2026-09-16 discovery also required `brama:agent:<agent>` per caller,
//! and that gate cost more than it protected: a subscription Brama could not
//! route turned out to be missing `brama:agent:weles`; on charless-mac-mini on
//! 2026-09-02 three of four accounts carried no agent tag; and on 2026-09-16
//! two of three Claude subscriptions on the operator's laptop were tagged for
//! nobody while the consumer `oko` was tagged on nothing, so six paid plans
//! answered `all bounded 'codex' credentials unavailable`. The operator's
//! word: a subscription in the vault serves every caller. The agent tag is
//! provenance now, and these tests hold the writer and the pool to that.

#[path = "../support/mod.rs"]
mod support;

use std::process::Command;

use serde_json::Value;

use brama::gateway::broker::subscription_tags_for_write;
use support::{SkarbiecVault, TestDirectory};

fn tags(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// The structural tags are derived from what the write is already for; an
/// existing provenance tag survives.
#[test]
fn the_write_supplies_the_structural_tags_it_can_derive() {
    let stored = subscription_tags_for_write(
        &tags(&["brama:agent:probierz"]),
        "codex",
        "brama-sub-wisent-app-codex-secondary",
    )
    .expect("an item is routable once marked and named, and must be written");

    assert!(
        stored.contains(&"brama:subscription".to_string()),
        "the subscription mark is what discovery filters on: {stored:?}"
    );
    assert!(
        stored.contains(&"brama:provider:codex".to_string()),
        "the provider is what this write is for: {stored:?}"
    );
    assert!(
        stored.contains(&"brama:id:brama-sub-wisent-app-codex-secondary".to_string()),
        "the subscription id is what this write is for: {stored:?}"
    );
    assert!(
        stored.contains(&"brama:agent:probierz".to_string()),
        "provenance must survive the write: {stored:?}"
    );
}

/// The exact state found on charless-mac-mini: provider and id present, no
/// mark and no agent. The write completes with the mark; nothing about an
/// agent is left for a writer to guess.
#[test]
fn a_write_with_no_agent_tag_completes_and_is_routable() {
    for existing in [
        tags(&[
            "brama:provider:codex",
            "brama:id:brama-sub-wisent-app-codex-secondary",
        ]),
        tags(&[]),
        tags(&["brama:agent:"]),
    ] {
        let stored =
            subscription_tags_for_write(&existing, "codex", "brama-sub-wisent-app-codex-secondary")
                .expect("no agent binding is required: every subscription serves every caller");
        assert!(
            stored.contains(&"brama:subscription".to_string()),
            "{stored:?}"
        );
        assert!(
            stored.contains(&"brama:provider:codex".to_string()),
            "{stored:?}"
        );
        assert!(
            stored.contains(&"brama:id:brama-sub-wisent-app-codex-secondary".to_string()),
            "{stored:?}"
        );
    }
}

/// The pool the real product reports from an isolated vault holds an account
/// tagged for nobody beside one tagged for an agent, both routable.
#[test]
fn an_account_tagged_for_no_agent_is_in_everyones_pool() {
    let directory = TestDirectory::new("pool-without-agent-tag");
    let vault = SkarbiecVault::create("pool-without-agent-tag");
    vault.seed_subscription("brama-pool-test", "codex", "pool-tagged-codex");
    vault.seed_marked_subscription("claude-code", "pool-untagged-claude");
    let (document, _) = pool_document(&directory, &vault);
    for id in ["pool-tagged-codex", "pool-untagged-claude"] {
        let row = row_of(&document, id);
        assert_eq!(row["status"], "active", "{id} must be in the pool: {row}");
    }
    assert!(
        document["unroutable"].as_array().map_or(true, |rows| rows
            .iter()
            .all(|row| row["id"] != "pool-untagged-claude")),
        "an account with the mark is never unroutable for want of an agent tag: {document}"
    );
}

/// The write never relabels an item that already claims a different provider or
/// subscription: that would silently move a paid plan onto another account.
#[test]
fn a_write_refuses_to_relabel_an_item_that_claims_something_else() {
    let refusal = subscription_tags_for_write(
        &tags(&["brama:agent:probierz", "brama:provider:claude-code"]),
        "codex",
        "brama-sub-wisent-app-codex-secondary",
    )
    .expect_err("a provider disagreement must be refused, not overwritten");
    assert!(refusal.contains("claude-code"), "{refusal}");

    let refusal = subscription_tags_for_write(
        &tags(&["brama:agent:probierz", "brama:id:some-other-subscription"]),
        "codex",
        "brama-sub-wisent-app-codex-secondary",
    )
    .expect_err("a subscription id disagreement must be refused, not overwritten");
    assert!(refusal.contains("some-other-subscription"), "{refusal}");
}

/// Every agent binding survives, not just the first: dropping one silently
/// unsubscribes that agent from a paid plan while every credential count stays
/// green.
#[test]
fn every_existing_agent_binding_survives_the_write() {
    let stored = subscription_tags_for_write(
        &tags(&[
            "brama:agent:wisent-app",
            "brama:agent:lem",
            "brama:agent:weles",
            "brama:agent:probierz",
        ]),
        "codex",
        "brama-sub-wisent-app-codex-primary",
    )
    .expect("a fully bound item must be written");

    for agent in ["wisent-app", "lem", "weles", "probierz"] {
        assert!(
            stored.contains(&format!("brama:agent:{agent}")),
            "{agent} must still be able to spend this plan: {stored:?}"
        );
    }
}

/// Read the real product's pool report from an isolated Skarbiec vault.
fn pool_document(directory: &TestDirectory, vault: &SkarbiecVault) -> (Value, bool) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    for (name, value) in vault.environment() {
        command.env(name, value);
    }
    let listed = command
        .env("HOME", directory.path().join("home"))
        .env("XDG_STATE_HOME", directory.path().join("xdg-state"))
        .env("BRAMA_STATE_DIR", directory.path().join("state"))
        .env(
            "BRAMA_SUBSCRIPTION_USAGE_FILE",
            directory.path().join("usage.json"),
        )
        .env("BRAMA_PERF_PATH", directory.path().join("perf.json"))
        .env("ENTITLEMENTS_ROUTER_BIN", vault.router())
        .args(["subscriptions", "--json"])
        .output()
        .expect("brama subscriptions --json");
    let document = serde_json::from_slice(&listed.stdout).unwrap_or_else(|error| {
        panic!(
            "the pool document must be JSON ({error}); stderr:\n{}",
            String::from_utf8_lossy(&listed.stderr)
        )
    });
    (document, listed.status.success())
}

fn row_of<'a>(document: &'a Value, subscription_id: &str) -> &'a Value {
    document["subscriptions"]
        .as_array()
        .expect("the document lists subscriptions")
        .iter()
        .find(|row| row["id"] == subscription_id)
        .unwrap_or_else(|| panic!("{subscription_id} is not in the document: {document}"))
}

/// Inventory alone is not evidence that authentication failed. Missing an
/// optional tag must not invent a refusal before any account was resolved.
#[test]
fn inventory_does_not_invent_an_authentication_failure() {
    let directory = TestDirectory::new("subscription-unobserved");
    let vault = SkarbiecVault::create("subscription-unobserved");
    vault.seed_subscription(
        "brama-account-observation",
        "codex",
        "subscription-unobserved",
    );
    let (document, _) = pool_document(&directory, &vault);
    let state = &row_of(&document, "subscription-unobserved")["automatic_sign_in"];
    assert_eq!(state["state"], "not_observed");
    assert_eq!(state["blocked_by"], Value::Null);
    assert!(document["errors"]
        .as_array()
        .unwrap()
        .iter()
        .all(|error| error["failure_point"] != "brama.subscriptions.automatic-sign-in"));
}

/// The narrowing that keeps healthy accounts from looking broken: nobody signs
/// an API key in, so an `openai` account is not awaiting anything, and the
/// console prints nothing about it.
#[test]
fn an_api_key_account_is_not_awaiting_a_sign_in() {
    let directory = TestDirectory::new("login-tag-api-key");
    let vault = SkarbiecVault::create("login-tag-api-key");
    let agent = "brama-login-tag";
    vault.seed_subscription(agent, "openai", "login-tag-openai");

    let (document, _) = pool_document(&directory, &vault);
    let state = &row_of(&document, "login-tag-openai")["automatic_sign_in"];
    assert_eq!(
        state["applies"],
        Value::Bool(false),
        "Weles signs in claude-code, codex and kimi, and this is none of them: {state}"
    );
    assert_eq!(
        state["blocked_by"],
        Value::Null,
        "an account nobody signs in has nothing blocking a sign-in: {state}"
    );
}
