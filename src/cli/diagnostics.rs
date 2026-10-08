//! `brama version`, `brama detect`, `brama test` and
//! `brama tasks measure|show|list`: what this build is, what this machine can
//! run, what the configured routes actually answer, and the named tasks
//! `task:<KEY>` selection serves from recorded checks.

use std::collections::BTreeMap;

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use brama::subscription_dispatch::{collect_task_quality as run_task_quality, TaskQualityOptions};
use brama::{detect_compute_resources, Message, ModelRequest};

#[derive(Args)]
pub(crate) struct TestArgs {
    /// Canonical provider/model route to test; required, no built-in route
    #[arg(short, long)]
    model: String,
    /// Jeden agent/client id whose provider credential should be used; required
    #[arg(long)]
    agent_id: String,
    /// Acknowledge that this command performs a billable provider request
    #[arg(long, default_value_t = false)]
    allow_provider_cost: bool,
    /// Print the answer as JSON instead of lines
    #[arg(long, default_value_t = false)]
    json: bool,
}

/// The named tasks `task:<KEY>` selection serves: measure one, and read back
/// the checks it reads.
#[derive(Subcommand)]
pub(crate) enum TasksCommand {
    /// Send one deterministic check to each selected active provider route for task KEY
    Measure(MeasureArgs),
    /// Print the checks recorded for task KEY: the evidence `task:<KEY>` selection reads
    Show(ShowArgs),
    /// Every task an agent has recorded checks for
    List(ListArgs),
}

#[derive(Args)]
pub(crate) struct MeasureArgs {
    /// Task key later selected as model="task:<KEY>"
    task: String,
    /// Jeden agent/client id whose provider credentials should be checked
    #[arg(long)]
    agent_id: String,
    /// Prompt sent to each active stateless provider route
    #[arg(long)]
    prompt: String,
    /// Exact expected response for a full score
    #[arg(long)]
    expected_exact: Option<String>,
    /// Expected substring for a full score
    #[arg(long)]
    expected_contains: Option<String>,
    /// Record each check, so task:<KEY> selection reads it
    #[arg(long, default_value_t = false)]
    persist: bool,
    /// How many active models to check, each one a billable request. No
    /// count is assumed: the caller decides what it spends.
    #[arg(long)]
    max_models: usize,
    /// Acknowledge that this command performs billable provider requests
    #[arg(long, default_value_t = false)]
    allow_provider_cost: bool,
    /// Print the report as JSON instead of key: value lines
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Args)]
pub(crate) struct ShowArgs {
    /// Task key selected as model="task:<KEY>"
    task: String,
    /// Jeden agent/client id the checks were recorded for
    #[arg(long)]
    agent_id: String,
    /// Print the checks as JSON instead of key: value lines
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Args)]
pub(crate) struct ListArgs {
    /// Jeden agent/client id whose tasks are listed
    #[arg(long)]
    agent_id: String,
    /// Print the tasks as JSON instead of key: value lines
    #[arg(long, default_value_t = false)]
    json: bool,
}

/// A refusal printed as it is, ending the command unsuccessfully.
fn fail(detail: impl std::fmt::Display) -> ! {
    eprintln!("{detail}");
    std::process::exit(1)
}

/// One JSON line by default, which release tooling and the docs read; with
/// `--text`, the same fields as `key: value` lines for a person.
pub(crate) fn print_version(text: bool) {
    let identity = serde_json::to_value(brama::build_info()).unwrap_or_default();
    if text {
        super::print_answer(&identity, false);
    } else {
        println!("{identity}");
    }
}

pub(crate) fn detect(json: bool) {
    let res = detect_compute_resources();
    if json {
        let report = serde_json::json!({ "resources": res });
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_else(|_| "{}".into())
        );
        return;
    }
    println!("GPU Type: {}", res.gpu_type.as_deref().unwrap_or("none"));
    println!("GPU Name: {}", res.gpu_name.as_deref().unwrap_or("unknown"));
    println!("VRAM: {:.1} GB", res.vram_gb);
    println!("RAM: {:.1} GB", res.ram_gb);
    println!("CPU Cores: {}", res.cpu_cores);
    println!("CUDA: {}", res.has_cuda);
    println!("Metal: {}", res.has_metal);
}

pub(crate) async fn test_inference(args: TestArgs) {
    let TestArgs {
        model,
        agent_id,
        allow_provider_cost,
        json,
    } = args;
    if !allow_provider_cost {
        eprintln!("refusing billable inference without explicit --allow-provider-cost");
        std::process::exit(2);
    }
    let request = ModelRequest {
        messages: vec![Message {
            role: "user".into(),
            content: "Say hello in one sentence.".into(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }],
        model,
        max_tokens: None,
        temperature: None,
        system: None,
        tools: None,
        tool_choice: None,
        billing_target: None,
    };
    let resp =
        brama::subscription_dispatch::dispatch_subscription_for_agent(&agent_id, &request).await;
    if resp.success {
        let answer = serde_json::json!({
            "model": resp.model,
            "response": resp.content,
            "input_tokens": resp.input_tokens,
            "output_tokens": resp.output_tokens,
            "latency_ms": resp.latency_ms,
            "cost_usd": resp.cost,
        });
        super::print_answer(&answer, json);
    } else {
        eprintln!("Error: {}", resp.error.unwrap_or_default());
        std::process::exit(1);
    }
}

/// `brama tasks measure|show|list`.
pub(crate) async fn tasks(command: TasksCommand) {
    match command {
        TasksCommand::Measure(args) => measure(args).await,
        TasksCommand::Show(args) => show(args),
        TasksCommand::List(args) => list(args),
    }
}

async fn measure(args: MeasureArgs) {
    let MeasureArgs {
        task,
        agent_id,
        prompt,
        expected_exact,
        expected_contains,
        persist,
        max_models,
        allow_provider_cost,
        json,
    } = args;
    if !allow_provider_cost {
        eprintln!(
            "refusing billable task-quality collection without explicit --allow-provider-cost"
        );
        std::process::exit(2);
    }
    match run_task_quality(TaskQualityOptions {
        agent_id,
        task,
        prompt,
        expected_exact,
        expected_contains,
        persist,
        max_models,
        allow_provider_cost,
    })
    .await
    {
        Ok(value) => super::print_answer(&value, json),
        Err(e) => fail(e),
    }
}

/// The checks recorded for one task, as `task:<KEY>` selection reads them;
/// a task nobody measured is refused, since selection refuses it too.
fn show(args: ShowArgs) {
    let checks = brama::journal::checks_for_task(&args.agent_id, &args.task);
    if checks.is_empty() {
        fail(format!(
            "no checks are recorded for task {} of agent {}; `brama tasks measure {} --agent-id {} --persist …` records them",
            args.task, args.agent_id, args.task, args.agent_id
        ));
    }
    super::print_answer(
        &json!({ "agent_id": args.agent_id, "task": args.task, "checks": checks }),
        args.json,
    );
}

/// Every task one agent has checks for: how many, and when the newest was taken.
fn list(args: ListArgs) {
    let mut tasks: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for check in brama::journal::checks_for_agent(&args.agent_id) {
        let (Some(task), Some(at)) = (check["task"].as_str(), check["checked_at"].as_str()) else {
            fail(format!(
                "the journal holds a check without its task or time: {check}"
            ));
        };
        tasks
            .entry(task.to_string())
            .or_default()
            .push(at.to_string());
    }
    let listed: Vec<Value> = tasks
        .into_iter()
        .map(|(task, times)| json!({ "task": task, "checks": times.len(), "newest": times.iter().max() }))
        .collect();
    super::print_answer(
        &json!({ "agent_id": args.agent_id, "tasks": listed }),
        args.json,
    );
}
