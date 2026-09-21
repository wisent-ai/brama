//! The promise the alias makes: a real model answers, and every answer is
//! inside the schema the caller declared.

use serde_json::{json, Value};

use crate::fixture::{
    decision_route, declared_levels, declared_options, questions, RealGateway, CHOICE_QUESTION,
    DECISION_ALIAS, NOUL_QUESTION, SCORE_QUESTION, STATE,
};

#[test]
fn the_decision_alias_answers_typed_questions_over_a_real_provider() {
    let route = decision_route();
    let gateway = RealGateway::start("served", json!({DECISION_ALIAS: route.clone()}));
    let (status, body) =
        gateway.decide(&json!({"model": DECISION_ALIAS, "state": STATE, "questions": questions()}));
    assert!(
        status.is_success(),
        "the decision alias did not serve over {route}: HTTP {status}: {body}"
    );
    assert_eq!(body["model"], DECISION_ALIAS, "{body}");
    assert_eq!(body["engine"], "chat-distribution", "{body}");
    assert!(
        body["route"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "the answer names no route: {body}"
    );

    let noul = body
        .pointer(&format!("/answers/{NOUL_QUESTION}/noul"))
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no noul answer: {body}"));
    assert!(
        (0.0..=1.0).contains(&noul),
        "a noul is a probability: {body}"
    );

    let options = declared_options();
    let choice = body
        .pointer(&format!("/answers/{CHOICE_QUESTION}/choice"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no choice answer: {body}"));
    assert!(
        options.iter().any(|option| option == choice),
        "the model answered an option the question never declared: {body}"
    );
    let probabilities = body
        .pointer(&format!("/answers/{CHOICE_QUESTION}/probabilities"))
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("no distribution: {body}"));
    assert_eq!(probabilities.len(), options.len(), "{body}");
    let mass: f64 = probabilities
        .values()
        .map(|value| value.as_f64().unwrap_or_default())
        .sum();
    assert!(
        (mass - 1.0).abs() < 0.01,
        "the distribution does not sum to one: {body}"
    );

    let levels = declared_levels();
    let score = body
        .pointer(&format!("/answers/{SCORE_QUESTION}/score"))
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no score answer: {body}"));
    assert!(
        (0.0..=(levels - 1) as f64).contains(&score),
        "the score falls outside the rubric it was given: {body}"
    );
    assert_eq!(
        body.pointer(&format!("/answers/{SCORE_QUESTION}/legend"))
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(levels),
        "the answer carries the rubric it was graded against: {body}"
    );
    assert!(
        body.pointer("/usage/input_tokens")
            .and_then(Value::as_u64)
            .is_some_and(|tokens| tokens > 0),
        "a real decision states real token usage: {body}"
    );
}
