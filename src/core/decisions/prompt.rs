//! Asking a model that generates text for the one thing a decision is made of.
//!
//! A decision provider answers the question vocabulary itself. A chat model
//! does not, so Brama asks it only for the probability mass over the labels
//! each question declares, and computes the typed answer from that itself in
//! [`super::answers`]. The choice, the score and the confidence are therefore
//! Brama's arithmetic over the model's distribution, never a second thing the
//! model was asked to assert and could contradict.

use serde_json::Value;

use crate::types::{Message, ModelRequest};

use super::question::{DecisionRequest, Question};

/// The instruction the chat engine sends, kept in one place because it is part
/// of the contract: change it and every chat-served decision changes with it.
const SYSTEM_PROMPT: &str = "You are a decision engine. You read a state and answer each question \
by distributing probability over exactly the labels that question declares. Answer with one JSON \
object and nothing else: no prose, no code fence, no explanation. Its keys are the question keys, \
and each value is an object mapping every declared label of that question to a number between 0 \
and 1. The numbers for one question sum to 1. Use every label the question declares and no other \
label.";

/// Room for the answer object: the labels plus the JSON around them. Nothing
/// is generated but numbers, so this is small by construction.
const TOKENS_PER_LABEL: u32 = 12;
const TOKENS_OVERHEAD: u32 = 64;

/// The chat request that carries one decision to a text model.
pub fn chat_request(request: &DecisionRequest, route: &str) -> ModelRequest {
    let labels: u32 = request
        .questions
        .iter()
        .map(|(_, question)| question.labels().len() as u32)
        .sum();
    let max_tokens = TOKENS_OVERHEAD
        .saturating_add(TOKENS_PER_LABEL.saturating_mul(labels))
        .min(crate::core::server::MAX_OUTPUT_TOKENS);
    ModelRequest {
        messages: vec![Message {
            role: "user".to_string(),
            content: Value::String(user_prompt(request)),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }],
        model: route.to_string(),
        max_tokens,
        temperature: None,
        system: Some(SYSTEM_PROMPT.to_string()),
        tools: None,
        tool_choice: None,
        billing_target: None,
    }
}

fn user_prompt(request: &DecisionRequest) -> String {
    let mut prompt = String::with_capacity(512);
    prompt.push_str("STATE\n");
    prompt.push_str(&request.state_text());
    prompt.push_str("\n\nQUESTIONS\n");
    for (key, question) in &request.questions {
        prompt.push_str(&format!(
            "- {key} ({}): {}\n",
            question.kind(),
            question.instructions()
        ));
        match question {
            Question::Noul { .. } => {
                prompt.push_str("  labels: true, false\n");
            }
            Question::Choice { options, .. } => {
                prompt.push_str("  labels:\n");
                for (label, rubric) in options {
                    match rubric {
                        Some(rubric) => prompt.push_str(&format!("    {label}: {rubric}\n")),
                        None => prompt.push_str(&format!("    {label}\n")),
                    }
                }
            }
            Question::Score { levels, .. } => {
                prompt.push_str("  labels, lowest first:\n");
                for (index, level) in levels.iter().enumerate() {
                    prompt.push_str(&format!("    {index}: {level}\n"));
                }
            }
        }
    }
    prompt.push_str(
        "\nANSWER\nOne JSON object keyed by question key, each value an object of label probabilities.",
    );
    prompt
}
