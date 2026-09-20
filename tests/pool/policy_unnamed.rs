//! A subscription the vault holds that this process was not started with.
//!
//! The runtime policy is generated from the vault's tags when a release is
//! installed on a host, and the launcher builds the boot catalogue from the
//! subscriptions that policy names. An account added afterwards is in neither,
//! so the gateway has never heard of it: on 2026-09-20 `charless-mac-mini`
//! held three paid Claude accounts and two Codex accounts, tagged and routed,
//! and answered every request with `no working subscription model for signed
//! agent` while its readiness answer reported nothing at all.
//!
//! Run against a real isolated Skarbiec vault through the same entitlements
//! router the pool reads, with the boot catalogue this process was given.

#[path = "../support/mod.rs"]
mod support;

use std::path::Path;

use support::SkarbiecVault;

const PROVIDER: &str = "claude-code";
const NAMED: &str = "brama-sub-policy-named-primary";
const UNNAMED: &str = "brama-sub-policy-unnamed-secondary";

/// The vault, the router and the boot catalogue, exactly as a launched
/// gateway has them.
fn arrange(vault: &SkarbiecVault, catalogue: Option<&str>) {
    for (name, value) in vault.environment() {
        std::env::set_var(name, value);
    }
    std::env::set_var("ENTITLEMENTS_ROUTER_BIN", vault.router());
    match catalogue {
        Some(json) => std::env::set_var("BRAMA_SUBSCRIPTION_CATALOG", json),
        None => std::env::remove_var("BRAMA_SUBSCRIPTION_CATALOG"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_account_the_boot_catalogue_lacks_is_named_and_one_it_has_is_not() {
    let vault = SkarbiecVault::create("policy-unnamed");
    vault.seed_marked_subscription(PROVIDER, NAMED);
    vault.seed_marked_subscription(PROVIDER, UNNAMED);
    arrange(
        &vault,
        Some(&format!(
            r#"{{"items":[{{"id":"{NAMED}","provider":"{PROVIDER}","status":"active"}}]}}"#
        )),
    );

    let unnamed = brama::gateway::broker::policy_unnamed_subscriptions().await;
    let ids: Vec<&str> = unnamed.iter().map(|entry| entry.id.as_str()).collect();
    assert_eq!(
        ids,
        [UNNAMED],
        "the account the boot catalogue lacks is the one to report: {ids:?}"
    );
    assert_eq!(unnamed[0].provider, PROVIDER);

    // No catalogue at all is a process started without one - a standalone or a
    // test - where every account would look unnamed. That says nothing, so
    // nothing is reported.
    arrange(&vault, None);
    assert!(
        brama::gateway::broker::policy_unnamed_subscriptions()
            .await
            .is_empty(),
        "without a boot catalogue the census must stay silent"
    );

    // A catalogue that names both leaves nothing to report.
    arrange(
        &vault,
        Some(&format!(
            r#"{{"items":[{{"id":"{NAMED}","provider":"{PROVIDER}","status":"active"}},
                 {{"id":"{UNNAMED}","provider":"{PROVIDER}","status":"active"}}]}}"#
        )),
    );
    assert!(
        brama::gateway::broker::policy_unnamed_subscriptions()
            .await
            .is_empty(),
        "every seeded account is named by this catalogue"
    );
    assert!(
        Path::new(vault.router()).is_file(),
        "the census read the vault through the real router"
    );
}
