//! Turning what a model answered into the typed answer a caller receives.
//!
//! Both engines land here, and both are checked against what the caller
//! declared: an answer that names an option the question does not have, omits
//! a label, or carries no probability mass is refused by name. A decision
//! whose answer cannot be trusted to be inside its own schema is worth
//! nothing, so nothing is guessed and nothing is repaired silently.
//! Probabilities are returned as computed: no precision is imposed on them.

use serde_json::{json, Map, Value};

use super::question::{DecisionRequest, Question};

mod native;

/// The typed answers a text model's reply carries, or the exact sentence it is
/// refused with.
pub fn answers_from_text(
    request: &DecisionRequest,
    text: &str,
) -> Result<Map<String, Value>, String> {
    let body = json_object(text)?;
    check_question_keys(request, &body)?;
    let mut answers = Map::new();
    for (key, question) in &request.questions {
        let declared = body
            .get(key)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("answer for question `{key}` is missing"))?;
        let distribution = distribution(key, question, declared)?;
        answers.insert(key.clone(), typed_answer(key, question, &distribution)?);
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
    let answered = object
        .get("answers")
        .ok_or_else(|| "the provider answered without `answers`".to_string())?
        .as_object()
        .ok_or_else(|| "the provider's `answers` is not an object of answers".to_string())?;
    check_question_keys(request, answered)?;
    let mut answers = Map::new();
    for (key, question) in &request.questions {
        let answer = answered
            .get(key)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("answer for question `{key}` is missing"))?;
        answers.insert(key.clone(), native::native_answer(key, question, answer)?);
    }
    Ok(answers)
}

/// A structured answer cannot add questions the caller did not ask.
fn check_question_keys(
    request: &DecisionRequest,
    answered: &Map<String, Value>,
) -> Result<(), String> {
    for key in answered.keys() {
        if request.question(key).is_none() {
            return Err(format!("answer names undeclared question `{key}`"));
        }
    }
    Ok(())
}

/// The probabilities one question was answered with, normalized to sum to one.
fn distribution(
    key: &str,
    question: &Question,
    answered: &Map<String, Value>,
) -> Result<Vec<(String, f64)>, String> {
    let labels = question.labels();
    // A text model writes a label the way its tokenizer spells it, often with
    // the space that precedes a word (` acknowledgement`). The label it names is
    // the declared one all the same, so keys are read without surrounding
    // whitespace; a key that is still not declared is refused, and so are two
    // keys that name one label, because which mass the model meant is unknown.
    let mut named = std::collections::HashMap::with_capacity(answered.len());
    for (written, value) in answered {
        let label = written.trim();
        if !labels.iter().any(|declared| declared == label) {
            return Err(format!(
                "answer for question `{key}` names label `{written}`, which this question does not declare"
            ));
        }
        if named.insert(label, value).is_some() {
            return Err(format!(
                "answer for question `{key}` names label `{label}` more than once"
            ));
        }
    }
    let mut mass = Vec::with_capacity(labels.len());
    for label in &labels {
        // A label the model left out carries no mass: a model certain of one
        // label writes that label alone, and the distribution it means puts
        // all of the mass on what it named. A label written with something
        // that is not a number is refused: read as no mass it would move the
        // model's mass silently.
        let value = match named.get(label.as_str()) {
            None => None,
            Some(written) => Some(written.as_f64().ok_or_else(|| {
                format!("answer for question `{key}` gives label `{label}` {written}, which is not a number")
            })?),
        };
        if let Some(value) = value {
            if !value.is_finite() || value.is_sign_negative() {
                return Err(format!(
                    "answer for question `{key}` gives label `{label}` a probability that is not a non-negative number"
                ));
            }
        }
        mass.push((label.clone(), value));
    }
    let total: f64 = mass.iter().filter_map(|(_, value)| *value).sum();
    if !total.is_normal() {
        return Err(format!(
            "answer for question `{key}` carries no probability mass"
        ));
    }
    Ok(mass
        .into_iter()
        .map(|(label, value)| {
            let share = match value {
                Some(value) => value / total,
                None => 0.0, // https://en.wikipedia.org/wiki/Probability_axioms "P(∅) = 0": a label the model did not name is one it gave no mass
            };
            (label, share)
        })
        .collect())
}

/// The typed answer that follows from one distribution.
fn typed_answer(
    key: &str,
    question: &Question,
    distribution: &[(String, f64)],
) -> Result<Value, String> {
    let (top_label, top_mass) = distribution
        .iter()
        .max_by(|left, right| left.1.total_cmp(&right.1))
        .ok_or_else(|| format!("question `{key}` declares no label to answer with"))?;
    Ok(match question {
        Question::Noul { .. } => {
            let truth = distribution
                .iter()
                .find(|(label, _)| label == "true")
                .map(|(_, value)| *value)
                .ok_or_else(|| format!("yes-or-no question `{key}` has no `true` label"))?;
            json!({
                "noul": truth,
                "confidence": truth.max(complement(truth)),
            })
        }
        Question::Choice { .. } => json!({
            "choice": top_label,
            "probabilities": probability_map(distribution),
            "confidence": top_mass,
        }),
        Question::Score { levels, .. } => {
            let score = distribution
                .iter()
                .enumerate()
                .map(|(index, (_, value))| index as f64 * value)
                .sum::<f64>();
            json!({
                "score": score,
                "probabilities": probability_map(distribution),
                "confidence": top_mass,
                "legend": levels,
            })
        }
    })
}

fn probability_map(distribution: &[(String, f64)]) -> Map<String, Value> {
    distribution
        .iter()
        .map(|(label, value)| (label.clone(), json!(value)))
        .collect()
}

/// The probability of the other outcome of a yes-or-no question.
fn complement(probability: f64) -> f64 {
    1.0 - probability // https://en.wikipedia.org/wiki/Complementary_event "P(A') = 1 - P(A)"
}

/// A stated probability: a finite number in [0, 1].
fn probability(value: Option<&Value>) -> Option<f64> {
    let certain = 1.0; // https://en.wikipedia.org/wiki/Probability_axioms "P(Ω) = 1" bounds every probability
    value
        .and_then(Value::as_f64)
        .filter(|number| number.is_finite() && !number.is_sign_negative() && *number <= certain)
}

/// The complete JSON object of label probabilities a structured-output model
/// answered, or serde's cause and position when it did not.
fn json_object(text: &str) -> Result<Map<String, Value>, String> {
    serde_json::from_str(text).map_err(|error| {
        format!("the model's JSON object of label probabilities does not parse: {error}")
    })
}
