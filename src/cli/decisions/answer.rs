//! One typed answer on one line, in the same words the answer document uses.

use serde_json::{Map, Value};

pub(super) fn answer_line(answer: &Value) -> String {
    let Some(answer) = answer.as_object() else {
        return answer.to_string();
    };
    let confidence = answer
        .get("confidence")
        .and_then(Value::as_f64)
        .map(|value| format!("  confidence {value:.2}"))
        .unwrap_or_default();
    if let Some(noul) = answer.get("noul").and_then(Value::as_f64) {
        return format!("noul {noul:.4}{confidence}");
    }
    if let Some(choice) = answer.get("choice").and_then(Value::as_str) {
        return format!("choice {choice}{confidence}{}", distribution(answer));
    }
    if let Some(score) = answer.get("score").and_then(Value::as_f64) {
        return format!("score {score:.4}{confidence}{}", distribution(answer));
    }
    Value::Object(answer.clone()).to_string()
}

fn distribution(answer: &Map<String, Value>) -> String {
    let Some(probabilities) = answer.get("probabilities").and_then(Value::as_object) else {
        return String::new();
    };
    let mut parts = probabilities
        .iter()
        .map(|(label, value)| format!("{label}={:.2}", value.as_f64().unwrap_or_default()))
        .collect::<Vec<_>>();
    parts.sort();
    format!("  [{}]", parts.join(" "))
}
