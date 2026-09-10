//! Reading the pool report the way a caller does: the ids it answered, the
//! refusal envelope field by field, and the proof that a scoped answer was
//! narrowed rather than filtered.
#![allow(dead_code)]

use serde_json::Value;

pub fn answered_ids(report: &Value) -> Vec<String> {
    report["subscriptions"]
        .as_array()
        .expect("the pool answers a subscriptions array")
        .iter()
        .map(|row| {
            row["id"]
                .as_str()
                .expect("every pool row names its subscription")
                .to_owned()
        })
        .collect()
}

/// The refusal envelope, field by field, as a caller reads it.
pub fn refusal(body: &Value) -> (String, String, String) {
    let field = |name: &str| body["error"][name].as_str().unwrap_or_default().to_owned();
    (field("code"), field("type"), field("message"))
}

/// The capability answered from its declaration, and told this caller nothing
/// about an account it was not answered about: a failed inventory means the
/// declaration was not read at all, and a refusal naming an account outside
/// the narrowing would leak somebody else's account.
pub fn assert_pool_answered(report: &Value) {
    let answered = answered_ids(report);
    for error in report["errors"]
        .as_array()
        .expect("the pool answers an errors array")
    {
        let point = error["failure_point"].as_str().unwrap_or_default();
        assert!(
            point != "brama.subscriptions.discovery" && point != "brama.subscriptions.ledger",
            "the pool could not read its own declaration: {error}"
        );
        if let Some(subscription) = error
            .pointer("/context/subscription")
            .and_then(Value::as_str)
        {
            assert!(
                answered.iter().any(|id| id == subscription),
                "the pool named {subscription} to a caller it did not answer about: {report}"
            );
        }
    }
}
