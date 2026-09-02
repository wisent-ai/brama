//! Why a live subscription contributes no model, said out loud.
//!
//! On charless-mac-mini on 2026-09-02, `/readyz` reported all four of that
//! host's subscription credentials as `redeemable: true, "the credential
//! redeemed"`, and then reported claude-code and kimi as "active subscription,
//! no model discovered" for every agent while codex, in the same sweep on the
//! same host, contributed five models. Three explanations fit that sentence and
//! they have different owners: the provider answered with nothing, the gateway
//! had no catalogue configured for those providers, or the discovery path never
//! ran for them.
//!
//! Separating them took reading `discover_models`' branches by hand and
//! counting `static_models` entries, because no surface would say which. These
//! tests hold the two facts that separation rests on, so it cannot quietly stop
//! being true, and hold the sentence that now carries the answer.

use brama::core::server::unroutable_reason;
use brama::provider_registry;

/// The providers a subscription can be held for. `provider_requires_caller_identity`
/// names exactly these three.
const SUBSCRIPTION_PROVIDERS: [&str; 3] = ["claude-code", "codex", "kimi"];

/// The load-bearing fact: every subscription provider carries a static model
/// list, so discovery cannot return an empty list because a provider answered
/// with nothing. `discover_models` appends `descriptor.static_models` after the
/// live `/models` call and only errors when the combined list is empty, so an
/// empty result proves the failure happened *before* the fallback — at
/// credential derivation — which is a different owner from the provider.
///
/// If someone empties one of these lists, that deduction silently stops
/// holding and "no model discovered" becomes ambiguous again. This is the test
/// that fails instead.
#[test]
fn every_subscription_provider_has_a_static_model_fallback() {
    for id in SUBSCRIPTION_PROVIDERS {
        let descriptor = provider_registry::provider(id)
            .unwrap_or_else(|| panic!("{id} must be in the Wisent provider registry"));
        assert!(
            !descriptor.static_models.is_empty(),
            "{id} has no static model fallback, so an empty discovery result would no longer \
             prove the refusal happened before the provider was asked"
        );
    }
}

/// The counts that made the measurement readable: codex contributing exactly
/// five models on that host matched its static list exactly, which is what
/// showed the fallback path was the live one and that it works there.
#[test]
fn the_static_lists_are_the_ones_the_measurement_matched() {
    let codex = provider_registry::provider("codex").expect("codex descriptor");
    assert_eq!(
        codex.static_models.len(),
        5,
        "codex contributed 5 models on the measured host; a different static count would mean \
         that reading no longer identifies the fallback path"
    );
    for id in ["claude-code", "kimi"] {
        let descriptor = provider_registry::provider(id).expect("descriptor");
        assert!(
            !descriptor.static_models.is_empty(),
            "{id} would have contributed {} models had discovery reached the fallback, and it \
             contributed none",
            descriptor.static_models.len()
        );
    }
}

/// With nothing recorded, the sentence is the one it always was: this must not
/// start claiming a reason it does not have.
#[test]
fn no_recorded_refusal_leaves_the_headline_alone() {
    assert_eq!(
        unroutable_reason(&[]),
        "active subscription, no model discovered"
    );
}

/// With a refusal recorded, the sentence carries it, naming the subscription.
#[test]
fn a_recorded_refusal_is_named_in_the_reason() {
    let refusal = "brama-sub-wisent-app-claude-primary: Skarbiec item \
                   `provider:claude-code:brama-sub-wisent-app-claude-primary` holds a JSON \
                   object with fields [category, login_method, metadata, type], which carries \
                   no credential"
        .to_string();

    let reason = unroutable_reason(std::slice::from_ref(&refusal));

    assert!(
        reason.starts_with("active subscription, no model discovered"),
        "the headline stays, so an existing reader still matches: {reason}"
    );
    assert!(
        reason.contains("brama-sub-wisent-app-claude-primary"),
        "the reason must name which subscription refused: {reason}"
    );
    assert!(
        reason.contains("carries no credential"),
        "the reason must carry the refusal itself, which is what says whose problem it is: \
         {reason}"
    );
}

/// Two subscriptions of one provider both refusing are both named. Reporting
/// one and dropping the other is the same defect one level down.
#[test]
fn every_recorded_refusal_survives() {
    let refusals = vec![
        "sub-one: holds a JSON object with fields [type], which carries no credential".to_string(),
        "sub-two: credential unavailable".to_string(),
    ];

    let reason = unroutable_reason(&refusals);

    assert!(reason.contains("sub-one"), "{reason}");
    assert!(reason.contains("sub-two"), "{reason}");
}
