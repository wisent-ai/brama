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
    // Lowercase digits only: the routing coordinate slugs the id to
    // lowercase, and the vault tag must name the same id.
    let subscription_id = format!(
        "{provider}-acquired-{}",
        chrono::Utc::now().format("%Y%m%d%H%M%S")
    );
    let mut verdict = json!({
        "provider": provider,
        "reason": options.reason,
        "trigger": options.trigger.name(),
        "cap": shortage.cap,
        "accounts": shortage.accounts,
        "standings": shortage.standings,
        "plan_tier": shortage.plan_tier,
        "subscription_id": subscription_id,
    });
    let mut requested = verdict.clone();
    requested["result"] = json!(REQUESTED);
    requested["detail"] = json!("Weles was asked to buy the account");
    crate::journal::record_subscription_acquisition(&requested);
    let purchase = match weles::purchase(
        provider,
        &subscription_id,
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
        return finish(verdict, FAILED, "weles_execution_unconfirmed", detail, Value::Null);
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
    for field in ["account", "subscription_item", "login_item", "paid", "run_id"] {
        verdict[field] = answer[field].clone();
    }
    if answer["ok"].as_bool() != Some(true) {
        let failure = answer["failure"].clone();
        let code = match failure["code"].as_str() {
            Some(code) => code.to_owned(),
            None => "purchase_failed".to_string(),
        };
        let detail = match failure["message"].as_str() {
            Some(message) => message.to_owned(),
            None => format!("Weles refused the purchase: {answer}"),
        };
        verdict["failure"] = failure;
        return finish(verdict, FAILED, &code, detail, Value::Null);
    }
    let refresh = match pool::refresh_subscription(provider, &subscription_id, &options.reason).await
    {
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
