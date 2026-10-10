//! Buying the account the decision found missing: Weles buys it and stores
//! its grant, Brama proves the grant with a refresh, and the journal holds
//! the request and its end.

use super::weles;

use serde_json::{json, Value};

use crate::subscription_dispatch::pool;

use super::decide::Shortage;
use super::{AcquireOptions, ACQUIRED, FAILED, REQUESTED};

pub(super) async fn buy(options: &AcquireOptions, shortage: Shortage) -> Value {
    let provider = options.provider.as_str();
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut verdict = json!({
        "provider": provider,
        "reason": options.reason,
        "trigger": options.trigger.name(),
        "shortage": shortage.kind,
        "why": shortage.why,
        "cap": shortage.cap,
        "accounts": shortage.accounts,
        "standings": shortage.standings,
        "plan_tier": shortage.plan_tier,
        "request_id": request_id,
    });
    let mut requested = verdict.clone();
    requested["result"] = json!(REQUESTED);
    requested["detail"] = json!("Weles was asked to buy the account");
    crate::journal::record_subscription_acquisition(&requested);
    let purchase = match weles::purchase(
        provider,
        &request_id,
        &shortage.plan_tier,
        &options.reason,
        options.progress.as_ref(),
    )
    .await
    {
        Ok(purchase) => purchase,
        Err(detail) => return finish(verdict, FAILED, "weles_unreachable", detail, Value::Null),
    };
    verdict["weles"] = purchase.observed;
    verdict["weles_http_status"] = json!(purchase.http_status);
    if let Some(detail) = purchase.transport_failure {
        return finish(
            verdict,
            FAILED,
            "weles_execution_unconfirmed",
            detail,
            Value::Null,
        );
    }
    let Some(answer) = purchase.answer else {
        return finish(
            verdict,
            FAILED,
            "weles_execution_unconfirmed",
            "Weles answered the purchase without a result".to_string(),
            Value::Null,
        );
    };
    for field in [
        "account",
        "subscription_id",
        "subscription_item",
        "login_item",
        "paid",
        "run_id",
    ] {
        verdict[field] = answer[field].clone();
    }
    if answer["ok"].as_bool() != Some(true) {
        let failure = answer["failure"].clone();
        // A Weles that answers `not_found` for the purchase has no purchase
        // route at all: the release serving this gateway predates it, which
        // no retry or account changes.
        let route_absent = purchase.http_status == Some(reqwest::StatusCode::NOT_FOUND.as_u16());
        let code = match (failure["code"].as_str(), route_absent) {
            (Some(code), _) => code.to_owned(),
            (None, true) => "weles_purchase_route_absent".to_string(),
            (None, false) => "purchase_failed".to_string(),
        };
        let detail = match (failure["message"].as_str(), route_absent) {
            (Some(message), _) => message.to_owned(),
            (None, true) => format!(
                "the Weles serving this gateway answered HTTP 404 for its purchase route, so it \
                 predates account purchase; release weles-worker with POST /subscriptions/acquire \
                 to that host ({answer})"
            ),
            (None, false) => format!("Weles refused the purchase: {answer}"),
        };
        verdict["failure"] = failure;
        return finish(verdict, FAILED, &code, detail, Value::Null);
    }
    let Some(account) = answer["account"].as_str() else {
        return finish(
            verdict,
            FAILED,
            "acquired_account_unidentified",
            "Weles reported a purchase without its account address".into(),
            Value::Null,
        );
    };
    if let Err(detail) =
        crate::subscription_dispatch::discovery::provider_account(provider, account)
    {
        return finish(
            verdict,
            FAILED,
            "acquired_account_unidentified",
            detail,
            Value::Null,
        );
    }
    let subscription_id = format!("{}-{}", provider, crate::gateway::broker::slug(account));
    if answer["subscription_id"].as_str() != Some(subscription_id.as_str()) {
        return finish(verdict, FAILED, "acquired_member_mismatch",
            format!("Weles did not bank the purchased account under its provider-and-address member {subscription_id}"),
            Value::Null);
    }
    let refresh =
        match pool::refresh_subscription(provider, &subscription_id, &options.reason).await {
            Ok(refresh) => refresh,
            Err(detail) => {
                return finish(
                    verdict,
                    FAILED,
                    "acquired_grant_unproven",
                    format!(
                    "Weles bought the account and stored its grant, but Brama could not refresh \
                     it: {detail}"
                ),
                    Value::Null,
                )
            }
        };
    if refresh["result"].as_str() != Some("refreshed") {
        let detail = format!(
            "Weles bought the account and stored its grant, but Brama's refresh did not prove \
             it: {}",
            refresh["detail"]
        );
        return finish(verdict, FAILED, "acquired_grant_unproven", detail, refresh);
    }
    let detail = format!(
        "bought {} on plan {} as {subscription_id}, and Brama refreshed its grant",
        verdict["account"], shortage.plan_tier
    );
    finish(verdict, ACQUIRED, "acquired", detail, refresh)
}

fn finish(mut verdict: Value, result: &str, code: &str, detail: String, refresh: Value) -> Value {
    verdict["result"] = json!(result);
    verdict["code"] = json!(code);
    verdict["detail"] = json!(detail);
    verdict["refresh"] = refresh;
    crate::journal::record_subscription_acquisition(&verdict);
    verdict
}
