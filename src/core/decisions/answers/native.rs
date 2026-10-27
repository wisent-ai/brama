//! Validate typed provider answers without treating malformed optional fields as absent.

use serde_json::{json, Map, Value};

use super::{complement, distribution, probability, Question};

pub(super) fn native_answer(
    key: &str,
    question: &Question,
    answer: &Map<String, Value>,
) -> Result<Value, String> {
    let mut normalized = Map::new();
    match question {
        Question::Noul { .. } => {
            let raw = answer
                .get("noul")
                .ok_or_else(|| format!("answer for question `{key}` is missing `noul`"))?;
            let value = probability(Some(raw)).ok_or_else(|| {
                format!("answer for question `{key}` has an invalid `noul` probability")
            })?;
            normalized.insert("noul".to_string(), json!(value));
            // A yes-or-no answer's confidence is the mass on the side it
            // leans to; a provider that states one is taken at its word.
            let stated = confidence(key, answer)?;
            let confidence = match stated {
                Some(value) => value,
                None => value.max(complement(value)),
            };
            normalized.insert("confidence".to_string(), json!(confidence));
        }
        Question::Choice { options, .. } => {
            let raw = answer
                .get("choice")
                .ok_or_else(|| format!("answer for question `{key}` is missing `choice`"))?;
            let choice = raw
                .as_str()
                .ok_or_else(|| format!("answer for question `{key}` has a non-string `choice`"))?;
            if !options.iter().any(|(label, _)| label == choice) {
                return Err(format!(
                    "answer for question `{key}` names option `{choice}`, which this question does not declare"
                ));
            }
            normalized.insert("choice".to_string(), json!(choice));
            carry_over(key, question, answer, &mut normalized)?;
        }
        Question::Score { levels, .. } => {
            let raw = answer
                .get("score")
                .ok_or_else(|| format!("answer for question `{key}` is missing `score`"))?;
            let score = raw
                .as_f64()
                .ok_or_else(|| format!("answer for question `{key}` has a non-number `score`"))?;
            // A score indexes the rubric's levels, from the first to the last.
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
            carry_over(key, question, answer, &mut normalized)?;
            normalized.insert("legend".to_string(), json!(levels));
        }
    }
    Ok(Value::Object(normalized))
}

fn confidence(key: &str, answer: &Map<String, Value>) -> Result<Option<f64>, String> {
    answer
        .get("confidence")
        .map(|value| {
            probability(Some(value)).ok_or_else(|| {
                format!("answer for question `{key}` carries an invalid `confidence`")
            })
        })
        .transpose()
}

/// Optional probabilities are normalized and validated against the declared
/// labels; an invalid confidence is a refusal, not an absent field.
fn carry_over(
    key: &str,
    question: &Question,
    answer: &Map<String, Value>,
    normalized: &mut Map<String, Value>,
) -> Result<(), String> {
    if let Some(raw) = answer.get("probabilities") {
        let probabilities = raw.as_object().ok_or_else(|| {
            format!("answer for question `{key}` carries non-object `probabilities`")
        })?;
        let distribution = distribution(key, question, probabilities)?;
        let probabilities = distribution
            .into_iter()
            .map(|(label, mass)| (label, json!(mass)))
            .collect();
        normalized.insert("probabilities".to_string(), Value::Object(probabilities));
    }
    if let Some(value) = confidence(key, answer)? {
        normalized.insert("confidence".to_string(), json!(value));
    }
    Ok(())
}
