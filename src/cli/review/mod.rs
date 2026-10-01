//! `brama review`: one tool-using review of a change, asked of a Brama
//! gateway, with the verdict as the exit status.
//!
//! A repository's pull-request gate (the Supabase schema repositories are the
//! first) needs a model to read a diff, look things up in the files the diff
//! touches, and say approve or request changes. That loop is the same for every
//! repository; only the instructions, the request and the directory the model
//! may read differ, so those are the arguments and the loop lives here once.
//!
//! The model can read and search files under `--root` and nothing else, and it
//! ends the review by calling `submit_verdict` with a typed verdict: the gate
//! never reads a decision out of prose.

mod gateway;
mod tools;

use std::path::PathBuf;

use clap::Args;
use serde_json::{json, Value};

use gateway::Gateway;
use tools::{Tree, Verdict};

#[derive(Args)]
pub(crate) struct ReviewArgs {
    /// Base URL of the Brama gateway, e.g. https://brama.wisent.com
    #[arg(long)]
    gateway: String,
    /// Model alias the review is asked of
    #[arg(long)]
    model: String,
    /// File holding the reviewer's instructions (the system message)
    #[arg(long)]
    instructions: PathBuf,
    /// File holding what is reviewed: the change, its description, its diff
    #[arg(long)]
    request: PathBuf,
    /// Directory the model may read and search; nothing outside it is served
    #[arg(long)]
    root: PathBuf,
    /// Most model turns before the review is refused as unfinished
    #[arg(long)]
    max_turns: usize,
    /// File the review text is written to, for posting it where it belongs
    #[arg(long)]
    output: Option<PathBuf>,
    /// Print the outcome as one JSON document
    #[arg(long, default_value_t = false)]
    json: bool,
}

/// The exit status of a review that asked for changes, apart from a failure
/// to review at all (1), so a gate can tell "rejected" from "not reviewed".
const CHANGES_REQUESTED: i32 = 3;

pub(crate) async fn run(args: ReviewArgs) {
    match review(&args).await {
        Ok((verdict, text, turns)) => {
            if let Some(path) = &args.output {
                if let Err(error) = std::fs::write(path, &text) {
                    eprintln!(
                        "the review could not be written to {}: {error}",
                        path.display()
                    );
                    std::process::exit(1);
                }
            }
            if args.json {
                super::print_json(&json!({
                    "verdict": verdict.name(),
                    "review": text,
                    "turns": turns,
                }));
            } else {
                println!("{text}");
            }
            if verdict == Verdict::RequestChanges {
                std::process::exit(CHANGES_REQUESTED);
            }
        }
        Err(message) => {
            eprintln!("review failed: {message}");
            std::process::exit(1);
        }
    }
}

/// The gateway bearer, read from standard input: a secret never travels in
/// argv or the environment (cli.md rule 15).
fn bearer_from_stdin() -> Result<String, String> {
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
        .map_err(|error| format!("the gateway bearer could not be read from standard input: {error}"))?;
    let token = text.trim().to_string();
    if token.is_empty() {
        return Err("standard input carried no gateway bearer; pipe it in, e.g. `skarbiec get <item> --field token | brama review ...`".into());
    }
    Ok(token)
}

async fn review(args: &ReviewArgs) -> Result<(Verdict, String, usize), String> {
    let token = bearer_from_stdin()?;
    let read = |path: &PathBuf| {
        std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))
            .and_then(|text| {
                (!text.trim().is_empty())
                    .then_some(text)
                    .ok_or_else(|| format!("{} is empty", path.display()))
            })
    };
    let instructions = read(&args.instructions)?;
    let request = read(&args.request)?;
    let tree = Tree::open(&args.root)?;
    let gateway = Gateway::new(&args.gateway, &token, &args.model);
    let mut messages = vec![
        json!({"role": "system", "content": instructions}),
        json!({"role": "user", "content": request}),
    ];
    for turn in 1..=args.max_turns {
        let answer = gateway.complete(&messages, &tools::schema()).await?;
        let calls = answer
            .get("tool_calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let content = answer.get("content").cloned().unwrap_or(Value::Null);
        if calls.is_empty() {
            messages.push(json!({"role": "assistant", "content": content}));
            messages.push(json!({
                "role": "user",
                "content": "End the review by calling submit_verdict.",
            }));
            continue;
        }
        messages.push(json!({"role": "assistant", "content": content, "tool_calls": calls}));
        for call in &calls {
            if let Some(outcome) = tools::submitted(call)? {
                return Ok((outcome.0, outcome.1, turn));
            }
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call.get("id").cloned().unwrap_or(Value::Null),
                "content": tree.answer(call),
            }));
        }
    }
    Err(format!(
        "the model did not submit a verdict within {} turns",
        args.max_turns
    ))
}
