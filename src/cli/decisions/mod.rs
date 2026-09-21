//! `brama decide`: one typed decision from an operator shell, over the same
//! aliases, routes and engines `POST /v1/decisions` serves.
//!
//! It exists for the two moments the HTTP surface cannot cover: before a
//! gateway is running, when an operator is establishing whether a route
//! answers at all, and while diagnosing one that stopped. The refusals here
//! are the endpoint's own sentences, so what this prints is what a caller
//! would have been told.

mod answer;

use std::io::Read;

use clap::Args;
use serde_json::{json, Value};

use brama::core::decisions::{decide_for_agent, DecisionFailure, DecisionRequest};
use brama::core::server::{alias_routing, BEST_DECISION_ALIAS, DECISION_ALIAS};

use answer::answer_line;

#[derive(Args)]
pub(crate) struct DecideArgs {
    /// Decision alias to answer through: decision-model or best-decision-model
    #[arg(long, default_value = DECISION_ALIAS)]
    model: String,
    /// The state to decide on; `-` reads it from standard input, `@FILE` from a file
    #[arg(long)]
    state: String,
    /// The questions object, as `POST /v1/decisions` takes it; `-` reads standard input, `@FILE` a file
    #[arg(long)]
    questions: String,
    /// Jeden agent/client id whose subscription answers `best-decision-model`
    #[arg(long, default_value = "wisent-app")]
    agent_id: String,
    /// Print the answer document as JSON instead of lines
    #[arg(long, default_value_t = false)]
    json: bool,
    /// Acknowledge that this command performs a billable provider request
    #[arg(long, default_value_t = false)]
    allow_provider_cost: bool,
}

pub(crate) async fn decide(args: DecideArgs) {
    let DecideArgs {
        model,
        state,
        questions,
        agent_id,
        json: as_json,
        allow_provider_cost,
    } = args;
    if !allow_provider_cost {
        eprintln!("refusing a billable decision without explicit --allow-provider-cost");
        std::process::exit(1);
    }
    if model != DECISION_ALIAS && model != BEST_DECISION_ALIAS {
        eprintln!("--model must be `{DECISION_ALIAS}` or `{BEST_DECISION_ALIAS}`");
        std::process::exit(1);
    }
    let state = read_argument("state", &state);
    let questions: Value = match serde_json::from_str(&read_argument("questions", &questions)) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("--questions is not JSON: {error}");
            std::process::exit(1);
        }
    };
    let request = match DecisionRequest::parse(&json!({
        "model": model,
        "state": state,
        "questions": questions,
    })) {
        Ok(request) => request,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    };
    let route = match alias_routing(&model) {
        Ok((Some(route), _)) => route,
        Ok((None, diagnosis)) => {
            eprintln!(
                "{}",
                diagnosis
                    .reason
                    .unwrap_or_else(|| format!("alias `{model}` has no serviceable route"))
            );
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!("the alias table could not be read: {error}");
            std::process::exit(1);
        }
    };
    match decide_for_agent(&route, &agent_id, &request).await {
        Ok(outcome) => {
            if as_json {
                super::print_json(&json!({
                    "model": model,
                    "route": outcome.route,
                    "engine": outcome.engine,
                    "answers": outcome.answers,
                    "usage": {
                        "input_tokens": outcome.input_tokens,
                        "output_tokens": outcome.output_tokens,
                        "latency_ms": outcome.latency_ms,
                    },
                }));
                return;
            }
            println!("Model: {model}");
            println!("Route: {} ({})", outcome.route, outcome.engine);
            for (key, answer) in &outcome.answers {
                println!("{key:<24} {}", answer_line(answer));
            }
            println!(
                "Tokens: {} in / {} out",
                outcome.input_tokens, outcome.output_tokens
            );
            println!("Latency: {:.0}ms", outcome.latency_ms);
        }
        Err(failure) => {
            match &failure {
                DecisionFailure::Provider(_) => {
                    eprintln!("provider refused: {}", failure.message())
                }
                DecisionFailure::Contract(_) => {
                    eprintln!("decision_contract_violated: {}", failure.message())
                }
            }
            std::process::exit(1);
        }
    }
}

/// A literal value, a file with `@FILE`, or standard input with `-`.
fn read_argument(name: &str, value: &str) -> String {
    if value == "-" {
        let mut buffer = String::new();
        if let Err(error) = std::io::stdin().read_to_string(&mut buffer) {
            eprintln!("--{name} could not be read from standard input: {error}");
            std::process::exit(1);
        }
        return buffer;
    }
    if let Some(path) = value.strip_prefix('@') {
        match std::fs::read_to_string(path) {
            Ok(text) => return text,
            Err(error) => {
                eprintln!("--{name} could not be read from {path}: {error}");
                std::process::exit(1);
            }
        }
    }
    value.to_string()
}
