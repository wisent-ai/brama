//! Typed decisions: a state and a set of questions in, one typed answer per
//! question out.
//!
//! This is the second thing Brama serves, beside generation, and it exists
//! because a decision is not a short chat. A caller that needs a route, a
//! gate or a grade needs an answer its code can act on — a probability, one of
//! its own options, a level of its own rubric — and it needs the same answer
//! whichever model is behind the alias today. That is exactly what an alias is
//! for, so a decision is addressed the same way generation is: through
//! `decision-model` and `best-decision-model`.
//!
//! Two engines answer them, and the caller cannot tell which one did:
//!
//! - a provider that speaks the decision wire itself — TypeSafe AI's System
//!   One model, Jev — answers the questions natively ([`ENGINE_NATIVE`]);
//! - any chat model answers the probability mass over each question's declared
//!   labels, and Brama computes the typed answer from it ([`ENGINE_CHAT`]).
//!
//! The second engine is what makes the aliases serviceable on a deployment
//! that holds no TypeSafe credential: a comparable model answers the same
//! contract, and `GET /v1/aliases` says which route did.

pub mod answers;
pub mod prompt;
pub mod question;

use serde_json::{Map, Value};

use crate::providers::adapter::native_decision_route;
use crate::subscription_dispatch::{
    dispatch_best_subscription_for_agent, dispatch_direct, dispatch_direct_decision,
};
use crate::types::{ModelResponse, Refusal};

pub use question::{DecisionRequest, Question};

/// The provider answered the typed questions in its own wire.
pub const ENGINE_NATIVE: &str = "typesafe-systemone";
/// A chat model answered the label distribution and Brama typed the answer.
pub const ENGINE_CHAT: &str = "chat-distribution";

/// Milliseconds in a second, for a latency read off a monotonic clock.
const MS_PER_SECOND: f64 = 1_000.0; // https://www.bipm.org/en/measurement-units/si-prefixes milli = 10^-3

/// One decision, answered.
#[derive(Clone, Debug)]
pub struct DecisionOutcome {
    pub answers: Map<String, Value>,
    pub engine: &'static str,
    pub route: String,
    /// The tokens the provider stated it read and wrote; `None` when its
    /// answer stated no usage, never a zero it did not report.
    pub input_tokens: Option<u32>,
    pub output_tokens: Option<u32>,
    pub latency_ms: f64,
}

/// A reply the decision contract refuses, with what gave it.
#[derive(Clone, Debug)]
pub struct ContractBreach {
    /// The route that answered: the subscription model `best` selected, or
    /// the direct route the alias names.
    pub answered_by: String,
    /// Why the reply is outside the schema the caller declared.
    pub reason: String,
    /// The reply as the model wrote it; a native answer as its JSON body.
    pub reply: String,
}

impl ContractBreach {
    /// The refusal's sentence: which route answered, and why its reply is refused.
    pub fn sentence(&self) -> String {
        format!(
            "{} answered outside the declared schema: {}",
            self.answered_by, self.reason
        )
    }
}

/// Why a decision was not answered, and whether the provider was reached.
#[derive(Clone, Debug)]
pub enum DecisionFailure {
    /// The provider refused, or could not be reached: its own sentence and
    /// the class the refusal was stated with.
    Provider(Refusal),
    /// The provider answered, and the answer was not inside the schema the
    /// caller declared. Naming this separately is the point: a model that
    /// invents an option is a different fault from a provider that is down,
    /// and only one of them is repaired by trying again later. It names the
    /// route that answered and keeps the reply, so the fault can be read
    /// without reproducing it.
    Contract(ContractBreach),
}

impl DecisionFailure {
    pub fn message(&self) -> String {
        match self {
            DecisionFailure::Provider(refused) => refused.message.clone(),
            DecisionFailure::Contract(breach) => breach.sentence(),
        }
    }
}

/// Answer one decision on a route this gateway pays for itself: the native
/// decision wire when the route speaks it, a chat model otherwise.
pub async fn decide_on_direct_route(
    route: &str,
    request: &DecisionRequest,
) -> Result<DecisionOutcome, DecisionFailure> {
    if native_decision_route(route) {
        return decide_natively(route, request).await;
    }
    let call = prompt::chat_request(request, route);
    let response = dispatch_direct(&call).await;
    outcome_from_chat(route, request, response)
}

/// Answer one decision for a named agent: `best` is served from that agent's
/// own subscription, every other route from the deployment's capability.
///
/// This is the path a caller that has already proved who it is takes — the
/// gateway with a bearer-bound `agent_id`, and `brama decide` in an operator
/// shell, which spends the same accounts the gateway would.
pub async fn decide_for_agent(
    route: &str,
    agent_id: &str,
    request: &DecisionRequest,
) -> Result<DecisionOutcome, DecisionFailure> {
    if route != crate::core::server::BEST_ALIAS {
        return decide_on_direct_route(route, request).await;
    }
    let call = prompt::chat_request(request, route);
    let response = dispatch_best_subscription_for_agent(agent_id, &call, None).await;
    outcome_from_chat(route, request, response)
}

async fn decide_natively(
    route: &str,
    request: &DecisionRequest,
) -> Result<DecisionOutcome, DecisionFailure> {
    let mut payload = Map::new();
    payload.insert("state".to_string(), request.state.clone());
    payload.insert("questions".to_string(), native_questions(request));
    let started = std::time::Instant::now();
    let body = dispatch_direct_decision(route, payload)
        .await
        .map_err(DecisionFailure::Provider)?;
    let breach = |reason: String| {
        DecisionFailure::Contract(ContractBreach {
            answered_by: route.to_string(),
            reason,
            reply: body.to_string(),
        })
    };
    let answers = answers::answers_from_native(request, &body).map_err(breach)?;
    Ok(DecisionOutcome {
        answers,
        engine: ENGINE_NATIVE,
        route: route.to_string(),
        input_tokens: usage_field(&body, "input_tokens").map_err(breach)?,
        output_tokens: usage_field(&body, "output_tokens").map_err(breach)?,
        latency_ms: started.elapsed().as_secs_f64() * MS_PER_SECOND,
    })
}

/// The caller's questions in the decision wire's own shape.
fn native_questions(request: &DecisionRequest) -> Value {
    let mut questions = Map::new();
    for (key, question) in &request.questions {
        let mut declared = Map::new();
        declared.insert("type".to_string(), Value::String(question.kind().into()));
        declared.insert(
            "instructions".to_string(),
            Value::String(question.instructions().to_string()),
        );
        match question {
            Question::Noul { .. } => {}
            Question::Choice { options, .. } => {
                let criteria = options
                    .iter()
                    // An option without a rubric is stated as JSON null.
                    .map(|(label, rubric)| (label.clone(), Value::from(rubric.clone())))
                    .collect::<Map<String, Value>>();
                declared.insert("criteria".to_string(), Value::Object(criteria));
            }
            Question::Score { levels, .. } => {
                declared.insert(
                    "criteria".to_string(),
                    Value::Array(levels.iter().cloned().map(Value::String).collect()),
                );
            }
        }
        questions.insert(key.clone(), Value::Object(declared));
    }
    Value::Object(questions)
}

/// `usage.<field>` of a provider's answer: `None` when it stated none, and a
/// refusal naming the number when it does not fit the count's width.
fn usage_field(body: &Value, field: &str) -> Result<Option<u32>, String> {
    let Some(stated) = body
        .get("usage")
        .and_then(|usage| usage.get(field))
        .and_then(Value::as_u64)
    else {
        return Ok(None);
    };
    u32::try_from(stated).map(Some).map_err(|error| {
        format!("the provider stated usage.{field} {stated}, which no count holds: {error}")
    })
}

/// One chat answer, read as a decision.
///
/// Public because the caller-scoped path — `best-decision-model`, served from
/// the caller's own subscription — dispatches through subscription routing
/// with the caller's signature, and then lands here with what it received.
pub fn outcome_from_chat(
    route: &str,
    request: &DecisionRequest,
    mut response: ModelResponse,
) -> Result<DecisionOutcome, DecisionFailure> {
    if let Some(refusal) = response.refusal.take() {
        return Err(DecisionFailure::Provider(refusal));
    }
    let answered_by = if response.model.is_empty() {
        route.to_string()
    } else {
        response.model.clone()
    };
    let answers = answers::answers_from_text(request, &response.content).map_err(|reason| {
        DecisionFailure::Contract(ContractBreach {
            answered_by: answered_by.clone(),
            reason,
            reply: response.content.clone(),
        })
    })?;
    Ok(DecisionOutcome {
        answers,
        engine: ENGINE_CHAT,
        route: answered_by,
        input_tokens: response.usage.map(|usage| usage.input_tokens),
        output_tokens: response.usage.map(|usage| usage.output_tokens),
        latency_ms: response.latency_ms,
    })
}
