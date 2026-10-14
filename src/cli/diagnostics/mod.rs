//! `brama version`, `brama detect`, `brama test` and
//! `brama tasks measure|show|list`: what this build is, what this machine can
//! run, what the configured routes actually answer, and the named tasks
//! `task:<KEY>` selection serves from recorded checks.

use clap::Args;

use brama::{detect_compute_resources, Message, ModelRequest};

mod tasks;
pub(crate) use tasks::{tasks, TasksCommand};

#[derive(Args)]
pub(crate) struct TestArgs {
    /// Canonical provider/model route to test, or `best`, the selector the
    /// gateway walks for the agent; required, no built-in route
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

/// A refusal printed as it is, ending the command unsuccessfully.
fn fail(detail: impl std::fmt::Display) -> ! {
    eprintln!("{detail}");
    crate::cli::fail()
}

/// One JSON line by default, which release tooling and the docs read; with
/// `--text`, the same fields as `key: value` lines for a person.
pub(crate) fn print_version(text: bool) {
    let identity = match serde_json::to_value(brama::build_info()) {
        Ok(identity) => identity,
        Err(error) => fail(format!(
            "this build's identity cannot be written as JSON: {error}"
        )),
    };
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
        println!("{report:#}");
        return;
    }
    match res.gpu_type.as_deref() {
        Some(kind) => println!("GPU Type: {kind}"),
        None => {
            println!("GPU Type: none detected (not Apple Silicon, and nvidia-smi named no GPU)")
        }
    }
    match res.gpu_name.as_deref() {
        Some(name) => println!("GPU Name: {name}"),
        None => println!("GPU Name: not reported"),
    }
    match res.vram_gb {
        Some(vram) => println!("VRAM: {vram:.1} GB"),
        None => println!("VRAM: no dedicated memory reported"),
    }
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
        response_schema: None,
    };
    // `best` is the selector the HTTP edge walks for a signed agent, not a
    // route: the same walk here, so what the gateway would answer this agent
    // can be read from the command line without a listener.
    let prepared = brama::subscription_dispatch::dispatch::prepare_routing(
        &agent_id,
        request.model == brama::core::server::BEST_ALIAS,
    )
    .await;
    let resp = if let Err(error) = prepared {
        brama::types::ModelResponse::from_refusal(
            &request.model,
            brama::types::Refusal::new(
                error.class,
                format!(
                    "route {}: no live subscription; discovery failed: {}",
                    request.model, error.message
                ),
            ),
        )
    } else if request.model == brama::core::server::BEST_ALIAS {
        brama::subscription_dispatch::dispatch_best_subscription_for_agent(
            &agent_id, &request, None,
        )
        .await
    } else {
        brama::subscription_dispatch::dispatch_subscription_for_agent(&agent_id, &request).await
    };
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
        // The refusal as the HTTP edge would answer it: its stable code from
        // the class it was built with, the attempts it cost, and its sentence.
        let reason = match resp.error.as_deref() {
            Some(reason) => reason.to_string(),
            None => "the dispatch refused without a sentence".to_string(),
        };
        let code = match resp.failure_kind {
            Some(class) => class.contract_kind(),
            None => "unclassified",
        };
        if json {
            let refused = serde_json::json!({
                "model": resp.model,
                "code": code,
                "attempts": resp.attempts,
                "error": reason,
            });
            super::print_answer(&refused, json);
        }
        eprintln!("Error [{code}, attempts {}]: {reason}", resp.attempts);
        crate::cli::fail();
    }
}
