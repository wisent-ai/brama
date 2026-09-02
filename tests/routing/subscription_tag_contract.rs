//! A subscription credential write must not be able to mint an account that
//! discovery cannot see.
//!
//! Discovery finds an account by `brama:subscription` plus
//! `brama:agent:<agent>` (`parse_live_subscriptions`). An item missing either
//! is not a degraded account: it does not exist for any caller, while its
//! credential stays valid and every check that counts credentials keeps
//! answering green. That asymmetry is why this shape is expensive.
//!
//! It has now happened twice in this fleet. First a subscription Brama could
//! not route turned out to be missing `brama:agent:weles`, with four paid plans
//! invisible for the same reason. Then, measured on charless-mac-mini on
//! 2026-09-02, three of the four subscription accounts in that vault carried
//! `brama:provider:` and `brama:id:` and neither `brama:subscription` nor any
//! `brama:agent:` -- so every agent on the host could reach exactly one
//! credential, and one recorded block on it took the documentation gate of
//! every repository calling the shared workflow down. One of the three redeemed
//! on the first probe after its tags were restored: a working paid credential
//! had been invisible the whole time.
//!
//! The writer had no half of that contract: `put_subscription_credential`
//! passed `None` for tags, which keeps whatever the item already had and gives
//! a fresh item nothing. These tests are the third occurrence failing here
//! instead of in a pipeline.

use brama::gateway::broker::subscription_tags_for_write;

fn tags(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// The structural tags are derived from what the write is already for, so a
/// rotation onto an item that has its agent binding completes the rest itself.
#[test]
fn the_write_supplies_the_structural_tags_it_can_derive() {
    let stored = subscription_tags_for_write(
        &tags(&["brama:agent:probierz"]),
        "codex",
        "brama-sub-wisent-app-codex-secondary",
    )
    .expect("an item with an agent binding is routable and must be written");

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
        "an existing agent binding must survive the write: {stored:?}"
    );
}

/// The exact state found on charless-mac-mini: provider and id present, no
/// mark and no agent. A write must refuse rather than store it again.
#[test]
fn a_write_that_would_leave_the_account_unroutable_is_refused() {
    let refusal = subscription_tags_for_write(
        &tags(&[
            "brama:provider:codex",
            "brama:id:brama-sub-wisent-app-codex-secondary",
        ]),
        "codex",
        "brama-sub-wisent-app-codex-secondary",
    )
    .expect_err("an account no agent can route to must not be written");

    assert!(
        refusal.contains("brama:agent:"),
        "the refusal must name the tag that is missing: {refusal}"
    );
    assert!(
        refusal.contains("retag-vault-item"),
        "the refusal must name the command that repairs it: {refusal}"
    );
}

/// A brand new item carries nothing, which is the case that mints an invisible
/// paid account. It is the same refusal.
#[test]
fn a_brand_new_item_with_no_tags_is_refused() {
    let refusal = subscription_tags_for_write(&[], "kimi", "brama-sub-wisent-app-kimi-primary")
        .expect_err("a fresh item has no agent binding, so it cannot be written blind");
    assert!(
        refusal.contains("no 'brama:agent:<agent>' tag"),
        "{refusal}"
    );
}

/// An empty agent tag is not an agent binding. Without this, `brama:agent:`
/// alone would satisfy the check and restore the original defect.
#[test]
fn an_empty_agent_tag_does_not_count_as_a_binding() {
    let refusal = subscription_tags_for_write(
        &tags(&["brama:agent:"]),
        "codex",
        "brama-sub-wisent-app-codex-secondary",
    )
    .expect_err("an empty agent tag names no agent");
    assert!(refusal.contains("brama:agent:"), "{refusal}");
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
