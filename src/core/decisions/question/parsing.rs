//! Reading one question out of a caller-s body, and refusing it in a sentence
//! that names the question it is about: the key, the kind, its options or
//! levels, and the text fields each kind requires.

use serde_json::{Map, Value};

use super::{Question, MAX_INSTRUCTIONS_BYTES, MAX_KEY_BYTES, MAX_OPTIONS};



pub(super) fn validate_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > MAX_KEY_BYTES || key.trim() != key {
        return Err(format!(
            "question key `{key}` must be non-empty, trimmed and at most {MAX_KEY_BYTES} bytes"
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
    for field in object.keys() {
        if !matches!(field.as_str(), "type" | "instructions" | "criteria") {
            return Err(format!(
                "question `{key}` carries unknown field `{field}`: a question has `type`, `instructions` and `criteria`"
            ));
        }
    }
    let instructions = text_field(object, key, "instructions")?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    match kind {
        "noul" => {
            if object.contains_key("criteria") {
                return Err(format!(
                    "question `{key}` is a noul and declares no criteria: a noul is answered with the probability that it is true"
                ));
            }
            Ok(Question::Noul { instructions })
        }
        "choice" => {
            let criteria = object.get("criteria").and_then(Value::as_object).ok_or_else(|| {
                format!("question `{key}` must declare `criteria` as an object of option labels")
            })?;
            if criteria.len() < 2 {
                return Err(format!(
                    "question `{key}` must declare at least two options"
                ));
            }
            if criteria.len() > MAX_OPTIONS {
                return Err(format!(
                    "question `{key}` must declare at most {MAX_OPTIONS} options"
                ));
            }
            let mut options = Vec::with_capacity(criteria.len());
            for (label, rubric) in criteria {
                validate_label(key, label)?;
                let rubric = match rubric {
                    Value::Null => None,
                    Value::String(text) if text.len() <= MAX_INSTRUCTIONS_BYTES => {
                        Some(text.clone())
                    }
                    Value::String(_) => {
                        return Err(format!(
                            "question `{key}` option `{label}` describes itself in more than {MAX_INSTRUCTIONS_BYTES} bytes"
                        ))
                    }
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
        "score" => {
            let criteria = object.get("criteria").and_then(Value::as_array).ok_or_else(|| {
                format!("question `{key}` must declare `criteria` as an ordered array of levels, lowest first")
            })?;
            if criteria.len() < 2 {
                return Err(format!("question `{key}` must declare at least two levels"));
            }
            if criteria.len() > MAX_OPTIONS {
                return Err(format!(
                    "question `{key}` must declare at most {MAX_OPTIONS} levels"
                ));
            }
            let mut levels = Vec::with_capacity(criteria.len());
            for level in criteria {
                let level = level.as_str().map(str::trim).filter(|text| {
                    !text.is_empty() && text.len() <= MAX_INSTRUCTIONS_BYTES
                });
                let Some(level) = level else {
                    return Err(format!(
                        "question `{key}` must describe every level with a non-empty string of at most {MAX_INSTRUCTIONS_BYTES} bytes"
                    ));
                };
                levels.push(level.to_string());
            }
            Ok(Question::Score {
                instructions,
                levels,
            })
        }
        "" => Err(format!(
            "question `{key}` must declare a `type`: noul, choice or score"
        )),
        other => Err(format!(
            "question `{key}` declares unsupported type `{other}`: a question is a noul, a choice or a score"
        )),
    }
}

fn text_field(object: &Map<String, Value>, key: &str, field: &str) -> Result<String, String> {
    let value = object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("question `{key}` must carry non-empty `{field}`"))?;
    if value.len() > MAX_INSTRUCTIONS_BYTES {
        return Err(format!(
            "question `{key}` field `{field}` must be at most {MAX_INSTRUCTIONS_BYTES} bytes"
        ));
    }
    Ok(value.to_string())
}

fn validate_label(key: &str, label: &str) -> Result<(), String> {
    if label.is_empty() || label.trim() != label || label.len() > MAX_KEY_BYTES {
        return Err(format!(
            "question `{key}` option `{label}` must be non-empty, trimmed and at most {MAX_KEY_BYTES} bytes"
        ));
    }
    if label.chars().any(char::is_control) {
        return Err(format!(
            "question `{key}` option `{label}` must not contain control characters"
        ));
    }
    Ok(())
}
