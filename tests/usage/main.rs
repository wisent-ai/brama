//! Subscription plan usage, driven as the three audiences that ask for it.
//!
//! One capability replaced four per-audience refreshes, and the claim worth
//! testing is that every audience is answered from one reading of one ledger
//! and one set of provider reports, narrowed by what it proved. These stories
//! drive the real released binary over real HTTP: an isolated vault behind the
//! entitlements router, an isolated Wisent Identity authority, and an isolated
//! endpoint answering the providers' own usage reports, because a vendor's
//! usage endpoint is not a fixture.
//!
//! ```console
//! $ cargo test --release --test usage -- --nocapture
//! ```

#[path = "../support/authority.rs"]
mod authority;
#[path = "fixture.rs"]
mod fixture;
#[path = "../support/gateway.rs"]
mod gateway;
#[path = "reports.rs"]
mod reports;
#[path = "../support/mod.rs"]
mod support;

use reqwest::Method;
use serde_json::{json, Value};

use authority::account_agent_id;
use fixture::{
    account, bank_the_agents_grant, error_for, provider_reports, row, vault, GRANT, OTHER,
    READABLE, REFUSING, UNPUBLISHED,
};
use gateway::{
    answered_ids, assert_pool_answered, refusal, Gateway, AGENT, AGENT_SIGNING_SECRET, OTHER_AGENT,
    PLAN_USAGE, STRANGER_BEARER,
};
use reports::{spawn_provider_reports, FIVE_HOUR_FRACTION, REFUSED_DETAIL};
use support::vault_item;

/// The console proves this installation, so it is answered about every plan the
/// deployment holds, and the reading comes from the provider's own report.
#[test]
fn plan_usage_answers_the_console_about_every_plan_the_deployment_holds() {
    let reports = spawn_provider_reports();
    let gateway = Gateway::start_with("usage-console", &vault(), &provider_reports(&reports));
    bank_the_agents_grant(&gateway);

    let (status, report) = gateway.console(PLAN_USAGE, Method::POST, None);
    assert_eq!(status, 200, "{report}");
    assert_pool_answered(&report);
    assert_eq!(report["scope"], "deployment");
    let mut answered = answered_ids(&report);
    answered.sort();
    assert_eq!(
        answered,
        vec![
            account(),
            READABLE.into(),
            REFUSING.into(),
            UNPUBLISHED.into(),
            OTHER.into()
        ],
    );

    let read = row(&report, READABLE);
    assert_eq!(read["usage_source"], "provider", "{read}");
    assert_eq!(read["stale"], false, "{read}");
    assert_eq!(read["usage_check"]["ok"], true, "{read}");
    let windows = read["limits"].as_array().expect("the plan windows");
    let named: Vec<&str> = windows
        .iter()
        .map(|window| window["limit_id"].as_str().expect("every window is named"))
        .collect();
    assert_eq!(named, vec!["anthropic:5h", "anthropic:7d"], "{read}");
    let used = windows[0]["used_fraction"]
        .as_f64()
        .expect("the window states what it has used");
    assert!(
        (used - FIVE_HOUR_FRACTION).abs() < 1e-9,
        "the five-hour window reports {used}, not what the provider stated: {read}"
    );
    assert!(
        windows[0]["resets_at_ms"].as_i64().is_some(),
        "a fraction with no reset instant cannot be aged: {read}"
    );
    assert!(
        reports
            .presented()
            .iter()
            .any(|presented| presented.contains(GRANT)),
        "the report was read without the account's own grant: {:?}",
        reports.presented()
    );
}

/// The signed agent proves an agent, and the same capability answers it about
/// its own plans only. Its bearer is model-scoped, which is the shape every
/// workload identity has, and the four routes this replaced refused that shape
/// outright.
#[test]
fn plan_usage_answers_a_signed_agent_only_about_its_own_plans() {
    let reports = spawn_provider_reports();
    let gateway = Gateway::start_with("usage-agent", &vault(), &provider_reports(&reports));
    bank_the_agents_grant(&gateway);

    let (status, report) = gateway.agent(PLAN_USAGE, Method::POST, None);
    assert_eq!(status, 200, "{report}");
    assert_pool_answered(&report);
    assert_eq!(report["scope"], AGENT);
    let mut answered = answered_ids(&report);
    answered.sort();
    assert_eq!(answered, vec![READABLE, REFUSING, UNPUBLISHED]);
    assert_eq!(row(&report, READABLE)["usage_source"], "provider");
}

/// The account holder proves a Wisent session, and the owner is derived from
/// that verified session rather than from anything the request carried.
#[test]
fn plan_usage_answers_an_account_holder_only_about_the_session_it_proved() {
    let reports = spawn_provider_reports();
    let gateway = Gateway::start_with("usage-account", &vault(), &provider_reports(&reports));

    let (status, report) = gateway.account(PLAN_USAGE, Method::POST, None);
    assert_eq!(status, 200, "{report}");
    assert_pool_answered(&report);
    assert_eq!(report["scope"], account_agent_id());
    assert_eq!(answered_ids(&report), vec![account()]);
}

/// A caller that proved nothing is told nothing, and a bearer that proves only
/// transport is not an audience.
#[test]
fn plan_usage_refuses_a_caller_that_proved_no_identity() {
    let reports = spawn_provider_reports();
    let gateway = Gateway::start_with("usage-unproven", &vault(), &provider_reports(&reports));

    let (status, body) = gateway.request(PLAN_USAGE, Method::POST, None, None, None, false);
    assert_eq!(status, 401, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "unauthenticated".to_owned(),
            "authentication_error".to_owned(),
            "unauthorized".to_owned(),
        ),
    );

    let (status, body) = gateway.request(
        PLAN_USAGE,
        Method::POST,
        Some(STRANGER_BEARER),
        None,
        None,
        false,
    );
    assert_eq!(status, 403, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "forbidden".to_owned(),
            "authorization_error".to_owned(),
            "forbidden".to_owned(),
        ),
    );
}

/// Two proofs that contradict each other are never resolved in favour of
/// either, and a caller has nothing to say here: the answer follows from the
/// identity, so a body is refused rather than signed over and ignored.
#[test]
fn plan_usage_refuses_contradicting_proofs_and_a_request_that_asks_for_something() {
    let reports = spawn_provider_reports();
    let gateway = Gateway::start_with("usage-wrong-owner", &vault(), &provider_reports(&reports));

    let (status, body) = gateway.request(
        PLAN_USAGE,
        Method::POST,
        Some(gateway::AGENT_BEARER),
        None,
        Some((OTHER_AGENT, AGENT_SIGNING_SECRET)),
        false,
    );
    assert_eq!(status, 403, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "forbidden".to_owned(),
            "authorization_error".to_owned(),
            "forbidden".to_owned(),
        ),
    );

    let (status, body) = gateway.console(
        PLAN_USAGE,
        Method::POST,
        Some(&json!({"agent_id": OTHER_AGENT})),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(
        refusal(&body),
        (
            "invalid_request".to_owned(),
            "request_error".to_owned(),
            "plan usage takes no request fields; the answer follows from the proven identity"
                .to_owned(),
        ),
    );
}

/// A report the provider would not give is an incomplete answer that says
/// which account it is incomplete about, in the provider's own words. It is
/// never a plan reported as unused, and an account whose provider publishes no
/// free report is a third, distinct statement.
#[test]
fn plan_usage_reports_a_provider_report_it_could_not_read() {
    let reports = spawn_provider_reports();
    let gateway = Gateway::start_with("usage-refused", &vault(), &provider_reports(&reports));
    bank_the_agents_grant(&gateway);

    let (status, report) = gateway.agent(PLAN_USAGE, Method::POST, None);
    assert_eq!(status, 200, "a refused report is not an admission failure");
    assert_eq!(report["ok"], false, "{report}");
    assert_pool_answered(&report);

    let refused = row(&report, REFUSING);
    assert_eq!(refused["usage_check"]["ok"], false, "{refused}");
    assert!(
        refused["limits"]
            .as_array()
            .expect("the plan windows")
            .is_empty(),
        "a refused report must not become a usage reading: {refused}"
    );
    assert!(refused["usage_source"].is_null(), "{refused}");
    let envelope = error_for(&report, REFUSING);
    let detail = envelope["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("returned HTTP 503") && detail.contains(REFUSED_DETAIL),
        "the refusal does not quote what the provider said: {envelope}"
    );
    assert_eq!(envelope["context"]["provider"], "kimi", "{envelope}");

    let unpublished = row(&report, UNPUBLISHED);
    assert_eq!(unpublished["usage_check"]["ok"], true, "{unpublished}");
    assert!(
        unpublished["limits"]
            .as_array()
            .expect("the plan windows")
            .is_empty(),
        "{unpublished}"
    );
    assert!(
        report["errors"]
            .as_array()
            .expect("plan usage answers an errors array")
            .iter()
            .all(|error| {
                error
                    .pointer("/context/subscription")
                    .and_then(Value::as_str)
                    != Some(UNPUBLISHED)
            }),
        "an unsupported report is not a failure: {report}"
    );
}

/// An account the ledger remembers and the vault no longer holds is reported
/// as exactly that. Its grant cannot be confirmed, so its usage is not
/// restated as current and the run says why.
#[test]
fn plan_usage_reports_an_account_the_vault_no_longer_holds() {
    let reports = spawn_provider_reports();
    let gateway = Gateway::start_with("usage-forgotten", &vault(), &provider_reports(&reports));
    bank_the_agents_grant(&gateway);

    let (status, first) = gateway.console(PLAN_USAGE, Method::POST, None);
    assert_eq!(status, 200, "{first}");
    assert_eq!(row(&first, READABLE)["usage_source"], "provider");

    gateway.rewrite_vault(&[
        vault_item(AGENT, "openai", UNPUBLISHED),
        vault_item(&account_agent_id(), "claude-code", &account()),
    ]);
    let (status, second) = gateway.console(PLAN_USAGE, Method::POST, None);
    assert_eq!(status, 200, "{second}");
    assert_eq!(second["ok"], false, "{second}");

    let forgotten = row(&second, READABLE);
    assert_eq!(forgotten["status"], "undiscovered", "{forgotten}");
    assert_eq!(forgotten["state"], "unknown", "{forgotten}");
    assert_eq!(
        error_for(&second, READABLE)["detail"],
        "subscription exists in usage history but was not returned by Skarbiec; its current \
         credential cannot be confirmed",
    );
}
