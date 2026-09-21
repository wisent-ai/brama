//! Every sentence a decision fails with, defended.
//!
//! An alias that fails quietly is worse than one that is missing, so each of
//! these reads the exact refusal an operator or a caller would act on.

use serde_json::{json, Value};

use crate::fixture::{
    decision_route, questions, RealGateway, DECISION_ALIAS, NOUL_QUESTION, STATE,
};

/// The chat story is refused before any generation, so its budget is nominal.
const REFUSED_CHAT_BUDGET: u32 = 16;

#[test]
fn an_unrouted_decision_alias_names_its_repair() {
    let gateway = RealGateway::start("unrouted", json!({}));
    let (status, body) =
        gateway.decide(&json!({"model": DECISION_ALIAS, "state": STATE, "questions": questions()}));
    assert_eq!(status.as_u16(), 503, "{body}");
    assert_eq!(body["error"]["code"], "alias_unserviceable", "{body}");
    assert_eq!(body["error"]["details"]["state"], "no_route", "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("brama routes set decision-model <provider/model>"),
        "the refusal does not name the repair: {body}"
    );
}

#[test]
fn a_malformed_decision_is_refused_before_any_provider() {
    let gateway = RealGateway::start("refusals", json!({DECISION_ALIAS: decision_route()}));

    let (status, body) = gateway
        .decide(&json!({"model": "wisent-backend", "state": STATE, "questions": questions()}));
    assert_eq!(status.as_u16(), 400, "{body}");
    assert_eq!(
        body["error"]["message"], "model must be `decision-model` or `best-decision-model`",
        "{body}"
    );

    let (status, body) = gateway.decide(&json!({
        "model": DECISION_ALIAS,
        "state": STATE,
        "questions": {"mood": {"type": "vibe", "instructions": "How does it feel?"}},
    }));
    assert_eq!(status.as_u16(), 400, "{body}");
    assert_eq!(
        body["error"]["message"],
        "question `mood` declares unsupported type `vibe`: a question is a noul, a choice or a score",
        "{body}"
    );

    let (status, body) = gateway.decide(&json!({
        "model": DECISION_ALIAS,
        "state": STATE,
        "questions": {
            "team": {"type": "choice", "instructions": "Who?", "criteria": {"only": null}},
        },
    }));
    assert_eq!(status.as_u16(), 400, "{body}");
    assert_eq!(
        body["error"]["message"], "question `team` must declare at least two options",
        "{body}"
    );

    let (status, body) =
        gateway.decide(&json!({"model": DECISION_ALIAS, "state": "", "questions": questions()}));
    assert_eq!(status.as_u16(), 400, "{body}");
    assert_eq!(
        body["error"]["message"], "`state` must not be empty",
        "{body}"
    );

    let (status, body) = gateway.decide(&json!({
        "model": DECISION_ALIAS,
        "state": STATE,
        "questions": {NOUL_QUESTION: {"instructions": "Does this convey urgency?"}},
    }));
    assert_eq!(status.as_u16(), 400, "{body}");
    assert_eq!(
        body["error"]["message"],
        format!("question `{NOUL_QUESTION}` must declare a `type`: noul, choice or score"),
        "{body}"
    );
}

/// A decision alias is not a chat model, and the chat endpoint says so rather
/// than answering that the caller's model name is wrong.
#[test]
fn the_chat_endpoint_refuses_a_decision_alias() {
    let gateway = RealGateway::start("chat-refusal", json!({DECISION_ALIAS: decision_route()}));
    let (status, body) = gateway.post(
        "/v1/chat/completions",
        &json!({
            "model": DECISION_ALIAS,
            "messages": [{"role": "user", "content": "hello"}],
            "max_tokens": REFUSED_CHAT_BUDGET,
        }),
    );
    assert_eq!(status.as_u16(), 400, "{body}");
    assert_eq!(
        body["error"]["message"],
        "`decision-model` is a decision alias: it answers typed questions on POST /v1/decisions, not chat completions",
        "{body}"
    );
    let _ = Value::Null;
}
