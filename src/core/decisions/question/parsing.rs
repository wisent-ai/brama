//! Reading one question out of a caller-s body, and refusing it in a sentence
//! that names the question it is about: the key, the kind, its options or
//! levels, and the text fields each kind requires.

use serde::Deserialize;
use serde_json::{Map, Value};

use super::Question;

/// The fields a question may carry. Reading a question into it refuses any
/// other field by name; what each field holds is judged below.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct QuestionFields {
    r#type: Option<Value>,
    instructions: Option<Value>,
    criteria: Option<Value>,
}

/// The kinds of question, as the caller spells them in `type`.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Noul,
    Choice,
    Score,
}

pub(super) fn validate_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.trim() != key {
        return Err(format!(
            "question key `{key}` must be non-empty and trimmed"
        ));
    }
    if key.chars().any(char::is_control) {
        return Err(format!(
            "question key `{key}` must not contain control characters"
        ));
    }
    Ok(())
}

pub(super) fn parse_question(key: &str, value: &Value) -> Result<Question, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("question `{key}` must be an object"))?;
    serde_json::from_value::<QuestionFields>(value.clone()).map_err(|error| {
        format!(
            "question `{key}`: {error}; a question has `type`, `instructions` and `criteria`"
        )
    })?;
    let instructions = text_field(object, key, "instructions")?;
    let kind = match object.get("type") {
        None => {
            return Err(format!(
                "question `{key}` must declare a `type`: noul, choice or score"
            ))
        }
        Some(declared) => serde_json::from_value::<Kind>(declared.clone()).map_err(|error| {
            format!(
                "question `{key}` declares unsupported type {declared}: a question is a noul, a choice or a score ({error})"
            )
        })?,
    };
    match kind {
        Kind::Noul => {
            if object.contains_key("criteria") {
                return Err(format!(
                    "question `{key}` is a noul and declares no criteria: a noul is answered with the probability that it is true"
                ));
            }
            Ok(Question::Noul { instructions })
        }
        Kind::Choice => {
            let criteria = object.get("criteria").and_then(Value::as_object).ok_or_else(|| {
                format!("question `{key}` must declare `criteria` as an object of option labels")
            })?;
            let declared: Vec<_> = criteria.iter().collect();
            if !matches!(declared.as_slice(), [_, _, ..]) {
                return Err(format!(
                    "question `{key}` must declare at least two options"
                ));
            }
            let mut options = Vec::with_capacity(declared.len());
            for (label, rubric) in declared {
                validate_label(key, label)?;
                let rubric = match rubric {
                    Value::Null => None,
                    Value::String(text) => Some(text.clone()),
                    _ => {
                        return Err(format!(
                            "question `{key}` option `{label}` must describe itself with a string or null"
                        ))
                    }
                };
                options.push((label.clone(), rubric));
            }
            Ok(Question::Choice {
                instructions,
                options,
            })
        }
        Kind::Score => {
            let criteria = object.get("criteria").and_then(Value::as_array).ok_or_else(|| {
                format!("question `{key}` must declare `criteria` as an ordered array of levels, lowest first")
            })?;
            if !matches!(criteria.as_slice(), [_, _, ..]) {
                return Err(format!("question `{key}` must declare at least two levels"));
            }
            let mut levels = Vec::with_capacity(criteria.len());
            for level in criteria {
                let level = level.as_str().map(str::trim).filter(|text| !text.is_empty());
                let Some(level) = level else {
                    return Err(format!(
                        "question `{key}` must describe every level with a non-empty string"
                    ));
                };
                levels.push(level.to_string());
            }
            Ok(Question::Score {
                instructions,
                levels,
            })
        }
    }
}

fn text_field(object: &Map<String, Value>, key: &str, field: &str) -> Result<String, String> {
    let value = object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("question `{key}` must carry non-empty `{field}`"))?;
    Ok(value.to_string())
}

fn validate_label(key: &str, label: &str) -> Result<(), String> {
    if label.is_empty() || label.trim() != label {
        return Err(format!(
            "question `{key}` option `{label}` must be non-empty and trimmed"
        ));
    }
    if label.chars().any(char::is_control) {
        return Err(format!(
            "question `{key}` option `{label}` must not contain control characters"
        ));
    }
    Ok(())
}
