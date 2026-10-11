//! `brama tasks measure|show|list`: the named tasks `task:<KEY>` selection
//! serves, measured against the configured routes and read back from the
//! checks recorded for them.

use std::collections::BTreeMap;

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use brama::subscription_dispatch::{collect_task_quality as run_task_quality, TaskQualityOptions};

use super::super::print_answer;
use super::fail;

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
        Ok(value) => print_answer(&value, json),
        Err(e) => fail(e),
    }
}

/// The checks recorded for one task, as `task:<KEY>` selection reads them;
/// a task nobody measured is refused, since selection refuses it too.
fn show(args: ShowArgs) {
    let checks = match brama::journal::checks_for_task(&args.agent_id, &args.task) {
        Ok(checks) => checks,
        Err(error) => fail(error),
    };
    if checks.is_empty() {
        fail(format!(
            "no checks are recorded for task {} of agent {}; `brama tasks measure {} --agent-id {} --persist …` records them",
            args.task, args.agent_id, args.task, args.agent_id
        ));
    }
    print_answer(
        &json!({ "agent_id": args.agent_id, "task": args.task, "checks": checks }),
        args.json,
    );
}

/// Every task one agent has checks for: how many, and when the newest was taken.
fn list(args: ListArgs) {
    let mut tasks: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let checks = match brama::journal::checks_for_agent(&args.agent_id) {
        Ok(checks) => checks,
        Err(error) => fail(error),
    };
    for check in checks {
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
    print_answer(
        &json!({ "agent_id": args.agent_id, "tasks": listed }),
        args.json,
    );
}
