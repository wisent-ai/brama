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

/// The typed answers a text model's reply carries, or the exact sentence it is
/// refused with.
pub fn answers_from_text(
    request: &DecisionRequest,
    text: &str,
) -> Result<Map<String, Value>, String> {
    let body = json_object(text)?;
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
    // A provider either wraps its answers in `answers` or sends them bare; an
    // `answers` that is not an object is refused rather than read as bare.
    let answered = match object.get("answers") {
        Some(Value::Object(wrapped)) => wrapped,
        Some(other) => {
            return Err(format!(
                "the provider's `answers` is {other}, not an object of answers"
            ))
        }
        None => object,
    };
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
            // A yes-or-no answer's confidence is the mass on the side it
            // leans to; a provider that states one is taken at its word.
            let confidence = match probability(answer.get("confidence")) {
                Some(stated) => stated,
                None => value.max(complement(value)),
            };
            normalized.insert("confidence".to_string(), json!(confidence));
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
            // A score indexes the rubric's levels, so it lies between the
            // first level's index and the last's.
            let Some(highest) = levels
                .iter()
                .enumerate()
                .last()
                .map(|(index, _)| index as f64)
            else {
                return Err(format!("question `{key}` declares no rubric levels"));
            };
            if !score.is_finite() || score.is_sign_negative() || score > highest {
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
    // A text model writes a label the way its tokenizer spells it, often with
    // the space that precedes a word (` acknowledgement`). The label it names is
    // the declared one all the same, so keys are read without surrounding
    // whitespace; a key that is still not declared is refused, and so are two
    // keys that name one label, because which mass the model meant is unknown.
    let mut named = Map::with_capacity(answered.len());
    for (written, value) in answered {
        let label = written.trim();
        if !labels.iter().any(|declared| declared == label) {
            return Err(format!(
                "answer for question `{key}` names label `{written}`, which this question does not declare"
            ));
        }
        if named.insert(label.to_string(), value.clone()).is_some() {
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
        let value = match named.get(label) {
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
    let probabilities = distribution
        .iter()
        .map(|(label, value)| (label.clone(), json!(value)))
        .collect::<Map<String, Value>>();
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
            "probabilities": probabilities,
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
                "probabilities": probabilities,
                "confidence": top_mass,
                "legend": levels,
            })
        }
    })
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

/// The first JSON object in a model's reply, read by serde from the first
/// brace on.
///
/// Models wrap JSON in prose or a code fence often enough that refusing the
/// answer for its packaging would refuse decisions that are perfectly well
/// formed inside. What follows the object is not read; an object that does
/// not parse is refused with serde's reason and position.
fn json_object(text: &str) -> Result<Map<String, Value>, String> {
    let start = text.find('{').ok_or_else(|| {
        "the model answered with no JSON object of label probabilities".to_string()
    })?;
    match serde_json::Deserializer::from_str(&text[start..])
        .into_iter::<Map<String, Value>>()
        .next()
    {
        Some(Ok(object)) => Ok(object),
        Some(Err(error)) => Err(format!(
            "the model's JSON object of label probabilities does not parse: {error}"
        )),
        None => Err("the model answered with no JSON object of label probabilities".to_string()),
    }
}
