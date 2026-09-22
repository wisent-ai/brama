//! What a caller may ask a decision model, and what it is refused for.
//!
//! The vocabulary is the one TypeSafe AI's System One model publishes for Jev
//! — a state plus a map of typed questions, each a `noul`, a `choice` or a
//! `score` — because callers that already speak it must not have to learn a
//! second shape to reach it through Brama. Everything here is validated before
//! any provider is contacted: a question this file rejects costs nothing and
//! names exactly what is wrong with it.

use serde_json::{Map, Value};

/// The most questions one call may carry. Every question is answered against
/// the same state in the same request, so this bounds one provider call.
pub const MAX_QUESTIONS: usize = 32;
/// The most options a choice may declare, or levels a score may grade over.
pub const MAX_OPTIONS: usize = 32;
/// The state a decision is taken on, as bytes of its JSON encoding.
pub const MAX_STATE_BYTES: usize = 131_072;
/// One instruction or rubric line.
pub const MAX_INSTRUCTIONS_BYTES: usize = 4_096;
/// One question key.
pub const MAX_KEY_BYTES: usize = 128;

/// One typed question, already validated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Question {
    /// A yes/no question answered with the probability that it is true.
    Noul { instructions: String },
    /// One option out of a declared set, each option carrying its own rubric.
    Choice {
        instructions: String,
        options: Vec<(String, Option<String>)>,
    },
    /// An ordered rubric, graded from its lowest level to its highest.
    Score {
        instructions: String,
        levels: Vec<String>,
    },
}

impl Question {
    /// The labels an answer to this question is allowed to carry, in the order
    /// they were declared. A score grades over positions, so its labels are the
    /// level indices and its rubric travels as the answer's legend.
    pub fn labels(&self) -> Vec<String> {
        match self {
            Question::Noul { .. } => vec!["true".to_string(), "false".to_string()],
            Question::Choice { options, .. } => {
                options.iter().map(|(label, _)| label.clone()).collect()
            }
            Question::Score { levels, .. } => {
                (0..levels.len()).map(|index| index.to_string()).collect()
            }
        }
    }

    pub fn instructions(&self) -> &str {
        match self {
            Question::Noul { instructions }
            | Question::Choice { instructions, .. }
            | Question::Score { instructions, .. } => instructions,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Question::Noul { .. } => "noul",
            Question::Choice { .. } => "choice",
            Question::Score { .. } => "score",
        }
    }
}

/// One decision call: the state every question is answered against, and the
/// questions themselves in the order the caller wrote them.
#[derive(Clone, Debug)]
pub struct DecisionRequest {
    pub model: String,
    pub state: Value,
    pub questions: Vec<(String, Question)>,
}

impl DecisionRequest {
    /// The caller's body, validated. `Err` is the exact sentence the caller is
    /// refused with, and it names the question it is about.
    pub fn parse(body: &Value) -> Result<Self, String> {
        let object = body
            .as_object()
            .ok_or_else(|| "the request body must be a JSON object".to_string())?;
        for field in object.keys() {
            if !matches!(field.as_str(), "model" | "state" | "questions") {
                return Err(format!(
                    "unknown field `{field}`: a decision carries `model`, `state` and `questions`"
                ));
            }
        }
        let model = object
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "missing field `model`".to_string())?
            .to_string();
        let state = object
            .get("state")
            .cloned()
            .ok_or_else(|| "missing field `state`".to_string())?;
        if state.is_null() || state.as_str().is_some_and(|text| text.trim().is_empty()) {
            return Err("`state` must not be empty".to_string());
        }
        if state.to_string().len() > MAX_STATE_BYTES {
            return Err(format!(
                "`state` must be at most {MAX_STATE_BYTES} bytes of JSON"
            ));
        }
        let declared = object
            .get("questions")
            .and_then(Value::as_object)
            .ok_or_else(|| "`questions` must be an object of question keys".to_string())?;
        if declared.is_empty() {
            return Err("`questions` must declare at least one question".to_string());
        }
        if declared.len() > MAX_QUESTIONS {
            return Err(format!(
                "`questions` must declare at most {MAX_QUESTIONS} questions"
            ));
        }
        let mut questions = Vec::with_capacity(declared.len());
        for (key, value) in declared {
            validate_key(key)?;
            questions.push((key.clone(), parse_question(key, value)?));
        }
        Ok(Self {
            model,
            state,
            questions,
        })
    }

    /// The state as the text a model reads.
    pub fn state_text(&self) -> String {
        match &self.state {
            Value::String(text) => text.clone(),
            other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
        }
    }

    pub fn question(&self, key: &str) -> Option<&Question> {
        self.questions
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, question)| question)
    }
}

mod parsing;

use parsing::{parse_question, validate_key};
