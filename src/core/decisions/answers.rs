//! Turning what a model answered into the typed answer a caller receives.
//!
//! Both engines land here, and both are checked against what the caller
//! declared: an answer that names an option the question does not have, omits
//! a label, or carries no probability mass is refused by name. A decision
//! whose answer cannot be trusted to be inside its own schema is worth
//! nothing, so nothing is guessed and nothing is repaired silently.

use serde_json::{json, Map, Value};

use super::question::{DecisionRequest, Question};

/// The typed answers a text model's reply carries, or the exact sentence it is
/// refused with.
pub fn answers_from_text(
    request: &DecisionRequest,
    text: &str,
) -> Result<Map<String, Value>, String> {
    let body = json_object(text).ok_or_else(|| {
        "the model answered with no JSON object of label probabilities".to_string()
    })?;
    let mut answers = Map::new();
    for (key, question) in &request.questions {
        let declared = body
            .get(key)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("answer for question `{key}` is missing"))?;
        let distribution = distribution(key, question, declared)?;
        answers.insert(key.clone(), typed_answer(question, &distribution));
    }
    Ok(answers)
}

/// A provider that speaks the decision wire answers in the typed vocabulary
/// already. What is checked here is that every question was answered and that
/// the answer stays inside what the question declared.
pub fn answers_from_native(
    request: &DecisionRequest,
    body: &Value,
) -> Result<Map<String, Value>, String> {
    let object = body
        .as_object()
        .ok_or_else(|| "the provider answered with no object of answers".to_string())?;
    // A provider may wrap its answers; both shapes are read the same way.
    let answered = object
        .get("answers")
        .and_then(Value::as_object)
        .unwrap_or(object);
    let mut answers = Map::new();
    for (key, question) in &request.questions {
        let answer = answered
            .get(key)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("answer for question `{key}` is missing"))?;
        answers.insert(key.clone(), native_answer(key, question, answer)?);
    }
    Ok(answers)
}

fn native_answer(
    key: &str,
    question: &Question,
    answer: &Map<String, Value>,
) -> Result<Value, String> {
    let mut normalized = Map::new();
    match question {
        Question::Noul { .. } => {
            let value = probability(answer.get("noul")).ok_or_else(|| {
                format!("answer for question `{key}` carries no `noul` probability")
            })?;
            normalized.insert("noul".to_string(), json!(value));
            normalized.insert(
                "confidence".to_string(),
                json!(probability(answer.get("confidence")).unwrap_or(value.max(1.0 - value))),
            );
        }
        Question::Choice { options, .. } => {
            let choice = answer
                .get("choice")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("answer for question `{key}` carries no `choice`"))?;
            if !options.iter().any(|(label, _)| label == choice) {
                return Err(format!(
                    "answer for question `{key}` names option `{choice}`, which this question does not declare"
                ));
            }
            normalized.insert("choice".to_string(), json!(choice));
            carry_over(answer, &mut normalized);
        }
        Question::Score { levels, .. } => {
            let score = answer
                .get("score")
                .and_then(Value::as_f64)
                .ok_or_else(|| format!("answer for question `{key}` carries no `score`"))?;
            if !score.is_finite() || score < 0.0 || score > (levels.len() - 1) as f64 {
                return Err(format!(
                    "answer for question `{key}` scores outside the rubric it was given"
                ));
            }
            normalized.insert("score".to_string(), json!(score));
            carry_over(answer, &mut normalized);
            normalized.insert("legend".to_string(), json!(levels));
        }
    }
    Ok(Value::Object(normalized))
}

/// The distribution and confidence a decision provider published, kept as it
/// answered them.
fn carry_over(answer: &Map<String, Value>, normalized: &mut Map<String, Value>) {
    if let Some(probabilities) = answer.get("probabilities").and_then(Value::as_object) {
        normalized.insert(
            "probabilities".to_string(),
            Value::Object(probabilities.clone()),
        );
    }
    if let Some(confidence) = probability(answer.get("confidence")) {
        normalized.insert("confidence".to_string(), json!(confidence));
    }
}

/// The probabilities one question was answered with, normalized to sum to one.
fn distribution(
    key: &str,
    question: &Question,
    answered: &Map<String, Value>,
) -> Result<Vec<(String, f64)>, String> {
    let labels = question.labels();
    for label in answered.keys() {
        if !labels.contains(label) {
            return Err(format!(
                "answer for question `{key}` names label `{label}`, which this question does not declare"
            ));
        }
    }
    let mut mass = Vec::with_capacity(labels.len());
    let mut total = 0.0;
    for label in &labels {
        // A label the model left out carries no mass. Refusing the whole
        // answer over it made the engine reject exactly the answers it asked
        // for: a model certain of one label writes that label alone, and the
        // distribution it means -- all of the mass on what it named, none on
        // the rest -- is the one computed here. An undeclared label is still
        // refused above, and an answer that names no declared label at all
        // carries no mass and is refused below.
        let value = answered.get(label).and_then(Value::as_f64).unwrap_or(0.0);
        if !value.is_finite() || value < 0.0 {
            return Err(format!(
                "answer for question `{key}` gives label `{label}` a probability that is not a number between zero and one"
            ));
        }
        total += value;
        mass.push((label.clone(), value));
    }
    if total <= 0.0 {
        return Err(format!(
            "answer for question `{key}` carries no probability mass"
        ));
    }
    for entry in mass.iter_mut() {
        entry.1 /= total;
    }
    Ok(mass)
}

/// The typed answer that follows from one distribution.
fn typed_answer(question: &Question, distribution: &[(String, f64)]) -> Value {
    let probabilities = distribution
        .iter()
        .map(|(label, value)| (label.clone(), json!(round(*value))))
        .collect::<Map<String, Value>>();
    let (top_label, top_mass) = distribution
        .iter()
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(label, value)| (label.clone(), *value))
        .unwrap_or_default();
    match question {
        Question::Noul { .. } => {
            let truth = distribution
                .iter()
                .find(|(label, _)| label == "true")
                .map(|(_, value)| *value)
                .unwrap_or_default();
            json!({
                "noul": round(truth),
                "confidence": round(truth.max(1.0 - truth)),
            })
        }
        Question::Choice { .. } => json!({
            "choice": top_label,
            "probabilities": probabilities,
            "confidence": round(top_mass),
        }),
        Question::Score { levels, .. } => {
            let score = distribution
                .iter()
                .enumerate()
                .map(|(index, (_, value))| index as f64 * value)
                .sum::<f64>();
            json!({
                "score": round(score),
                "probabilities": probabilities,
                "confidence": round(top_mass),
                "legend": levels,
            })
        }
    }
}

fn probability(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite() && *number >= 0.0 && *number <= 1.0)
}

fn round(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

/// The first complete JSON object in a model's reply.
///
/// Models wrap JSON in prose or a code fence often enough that refusing the
/// answer for its packaging would refuse decisions that are perfectly well
/// formed inside. Braces are counted outside string literals, so a brace in a
/// label cannot end the object early.
fn json_object(text: &str) -> Option<Map<String, Value>> {
    let bytes = text.as_bytes();
    let start = bytes.iter().position(|byte| *byte == b'{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str::<Map<String, Value>>(
                        text.get(start..=offset).unwrap_or_default(),
                    )
                    .ok();
                }
            }
            _ => {}
        }
    }
    None
}
