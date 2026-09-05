use std::io::Read;

use clap::{Parser, Subcommand};
use serde_json::Value;
use tracing::info;

use brama::subscription_dispatch::{collect_task_quality, TaskQualityOptions};
use brama::{
    detect_compute_resources, select_model_for_resources, start_server, Message, ModelRequest,
};

#[derive(Parser)]
#[command(name = "brama", about = "Multi-provider LLM router")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print secret-free product and build identity as JSON
    Version,
    /// Start the OpenAI-compatible HTTP server
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
        /// Read a standalone provider-to-credential JSON object from stdin
        #[arg(long, default_value_t = false)]
        local_credentials_stdin: bool,
    },
    /// Follow Brama's first-use journey and optionally receive one real model response
    Onboard {
        /// Canonical provider/model route for the first real response
        #[arg(short, long, default_value = "openai/default")]
        model: String,
        /// Stable workload id whose separately provisioned provider credential should be used
        #[arg(long, default_value = "wisent-app")]
        agent_id: String,
        /// Acknowledge that onboarding should perform one billable provider request
        #[arg(long, default_value_t = false)]
        allow_provider_cost: bool,
        /// Discard recorded progress and show the walkthrough again from its first step
        #[arg(long, default_value_t = false)]
        reset: bool,
    },
    /// Run a test inference through the router
    Test {
        /// Canonical provider/model route to test
        #[arg(short, long, default_value = "openai/default")]
        model: String,
        /// Jeden agent/client id whose provider credential should be used
        #[arg(long, default_value = "wisent-app")]
        agent_id: String,
        /// Acknowledge that this command performs a billable provider request
        #[arg(long, default_value_t = false)]
        allow_provider_cost: bool,
    },
    /// Detect local hardware capabilities
    Detect,
    /// Serve the read-only stdio MCP server (agent surface)
    Mcp,
    /// Report the subscription pool this gateway routes over
    Subscriptions {
        #[command(subcommand)]
        command: SubscriptionsCommand,
    },
    /// Report every model alias this gateway declares and whether it can serve
    Aliases {
        /// Print the report as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
        /// Exit non-zero when any declared alias cannot be served
        #[arg(long, default_value_t = false)]
        strict: bool,
    },
    /// Act on one provider's subscription credentials
    Subscription {
        #[command(subcommand)]
        command: SubscriptionCommand,
    },
    /// Collect deterministic task-quality checks for active provider routes
    CollectTaskQuality {
        /// Jeden agent/client id whose provider credentials should be checked
        #[arg(long)]
        agent_id: String,
        /// Task key used later as model="task:<task>"
        #[arg(long)]
        task: String,
        /// Prompt sent to each active stateless provider route
        #[arg(long)]
        prompt: String,
        /// Exact expected response for score=1
        #[arg(long)]
        expected_exact: Option<String>,
        /// Expected substring for score=1
        #[arg(long)]
        expected_contains: Option<String>,
        /// Write results into subscription_router_checks
        #[arg(long, default_value_t = false)]
        persist: bool,
        /// Maximum active models to check (bounded again by the library)
        #[arg(long, default_value = "3")]
        max_models: usize,
        /// Acknowledge that this command performs billable provider requests
        #[arg(long, default_value_t = false)]
        allow_provider_cost: bool,
    },
}

#[derive(Subcommand)]
enum SubscriptionsCommand {
    /// List every subscription in the pool with the state of its credential
    List {
        /// Print the report as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SubscriptionCommand {
    /// Refresh this provider's subscription credentials now
    Refresh {
        /// The provider whose grants should be refreshed (`codex`, `claude-code`, `kimi`)
        provider: String,
        /// Why this refresh is being run; recorded in the journal beside the verdict
        #[arg(long)]
        reason: String,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Sign one provider account in through Weles, then prove it by a refresh
    #[command(name = "sign-in")]
    SignIn {
        /// The provider whose account should be signed in (`codex`, `claude-code`, `kimi`)
        provider: String,
        /// The exact Weles sign-in row to drive; without it the single row Weles holds for the provider is used, and two or more are never guessed between
        #[arg(long)]
        login_item: Option<String>,
        /// Why this sign-in is being run; recorded in the journal beside the verdict
        #[arg(long)]
        reason: String,
        /// How long Weles may spend driving the browser, in milliseconds
        #[arg(long, default_value_t = 900_000)]
        login_timeout_ms: u64,
        /// Print the verdict as JSON instead of lines
        #[arg(long, default_value_t = false)]
        json: bool,
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Version => {
            println!(
                "{}",
                serde_json::to_string(&brama::build_info()).unwrap_or_else(|_| "{}".into())
            );
        }
        Commands::Serve {
            port,
            local_credentials_stdin,
        } => {
            if local_credentials_stdin {
                let mut encoded = String::new();
                if let Err(error) = std::io::stdin().read_to_string(&mut encoded) {
                    eprintln!("Server error: cannot read local credentials: {error}");
                    std::process::exit(1);
                }
                if let Err(error) =
                    brama::gateway::broker::install_local_provider_credentials(&mut encoded)
                {
                    eprintln!("Server error: {error}");
                    std::process::exit(1);
                }
            }
            info!("Starting server on port {port}");
            if let Err(e) = start_server(port, local_credentials_stdin).await {
                eprintln!("Server error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Onboard {
            model,
            agent_id,
            allow_provider_cost,
            reset,
        } => match brama::onboarding::run_first_use(model, agent_id, allow_provider_cost, reset)
            .await
        {
            Ok(false) if allow_provider_cost => {
                std::process::exit(1);
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!("Onboarding error: {error}");
                std::process::exit(1);
            }
        },
        Commands::Test {
            model,
            agent_id,
            allow_provider_cost,
        } => {
            if !allow_provider_cost {
                eprintln!("refusing billable inference without explicit --allow-provider-cost");
                std::process::exit(i32::from(true));
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
                max_tokens: 256,
                temperature: 0.7,
                system: None,
                tools: None,
                tool_choice: None,
                billing_target: None,
            };
            let resp =
                brama::subscription_dispatch::dispatch_subscription_for_agent(&agent_id, &request)
                    .await;
            if resp.success {
                println!("Model: {}", resp.model);
                println!("Response: {}", resp.content);
                println!(
                    "Tokens: {} in / {} out",
                    resp.input_tokens, resp.output_tokens
                );
                println!("Latency: {:.0}ms", resp.latency_ms);
                println!("Cost: ${:.6}", resp.cost);
            } else {
                eprintln!("Error: {}", resp.error.unwrap_or_default());
                std::process::exit(1);
            }
        }
        Commands::Detect => {
            let res = detect_compute_resources();
            println!("GPU Type: {}", res.gpu_type.as_deref().unwrap_or("none"));
            println!("GPU Name: {}", res.gpu_name.as_deref().unwrap_or("unknown"));
            println!("VRAM: {:.1} GB", res.vram_gb);
            println!("RAM: {:.1} GB", res.ram_gb);
            println!("CPU Cores: {}", res.cpu_cores);
            println!("CUDA: {}", res.has_cuda);
            println!("Metal: {}", res.has_metal);

            let (model, backend) = select_model_for_resources(&res);
            println!("\nRecommended model: {model}");
            println!("Recommended backend: {backend}");
        }
        Commands::Mcp => {
            brama::mcp::serve();
        }
        Commands::Subscriptions { command } => match command {
            SubscriptionsCommand::List { json } => {
                let report = brama::subscription_dispatch::pool::report().await;
                if json {
                    print_json(&report);
                } else {
                    print_pool(&report);
                }
            }
        },
        Commands::Aliases { json, strict } => {
            let report = match brama::core::server::alias_report() {
                Ok(report) => report,
                Err(error) => {
                    eprintln!("aliases could not be read: {error}");
                    std::process::exit(1);
                }
            };
            let unserviceable = report.unserviceable();
            if json {
                print_json(&serde_json::json!({
                    "source": report.source,
                    "aliases": report.aliases,
                    "unserviceable": unserviceable,
                }));
            } else {
                match &report.source.routes_file {
                    Some(path) => println!("route registry: {}", path.display()),
                    None => println!("route registry: none configured"),
                }
                println!(
                    "launcher alias table: {}",
                    if report.source.launcher_table_present {
                        "present in this process"
                    } else {
                        "absent; only the compiled-in contract and the route registry are visible here"
                    }
                );
                for alias in &report.aliases {
                    let chain = match &alias.route {
                        Some(route) if alias.fallbacks.is_empty() => route.clone(),
                        Some(route) => format!("{route} -> {}", alias.fallbacks.join(" -> ")),
                        None => "-".to_string(),
                    };
                    println!("{:<32} {:<20} {}", alias.alias, alias.state, chain);
                    if let Some(reason) = &alias.reason {
                        println!("{:<32} {}", "", reason);
                    }
                }
                println!(
                    "{} alias(es), {} cannot be served",
                    report.aliases.len(),
                    unserviceable
                );
            }
            if strict && unserviceable > 0 {
                std::process::exit(1);
            }
        }
        Commands::Subscription { command } => match command {
            SubscriptionCommand::Refresh {
                provider,
                reason,
                json,
            } => match brama::subscription_dispatch::pool::refresh_provider(&provider, &reason)
                .await
            {
                Ok(verdict) => {
                    if json {
                        print_json(&verdict);
                    } else {
                        print_refresh(&verdict);
                    }
                    // A refresh that obtained nothing exits non-zero after
                    // reporting, because the caller that runs this is trying to
                    // repair an empty pool and needs to know from the status
                    // whether it is still empty.
                    if text(&verdict, "result") != Some("refreshed") {
                        std::process::exit(1);
                    }
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            },
            SubscriptionCommand::SignIn {
                provider,
                login_item,
                reason,
                login_timeout_ms,
                json,
            } => match brama::subscription_dispatch::sign_in::sign_in_provider(
                brama::subscription_dispatch::sign_in::SignInOptions {
                    provider,
                    login_item,
                    subscription_id: None,
                    reason,
                    login_timeout_ms,
                },
            )
            .await
            {
                Ok(verdict) => {
                    if json {
                        print_json(&verdict);
                    } else {
                        print_sign_in(&verdict);
                    }
                    // A sign-in that did not end in a refreshed credential
                    // exits non-zero after reporting, because the caller is
                    // repairing a refused subscription and needs to know from
                    // the status whether it is still refused.
                    if text(&verdict, "result") != Some("signed_in") {
                        std::process::exit(1);
                    }
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            },
        },
        Commands::CollectTaskQuality {
            agent_id,
            task,
            prompt,
            expected_exact,
            expected_contains,
            persist,
            max_models,
            allow_provider_cost,
        } => {
            match collect_task_quality(TaskQualityOptions {
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
                Ok(value) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into())
                    );
                }
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
    }
}

/// One report as the desktop console consumes it.
fn print_json(report: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".into())
    );
}

/// The subscription pool as lines a person reads.
///
/// The count leads because it is the question that gets this command run: how
/// many credentials can still serve a `best` call. Nothing but a state, a
/// provider and an id is printed per row unless there is something more to say,
/// so a healthy pool is short and a broken one is where the sentences are.
fn print_pool(report: &Value) {
    let rows: &[Value] = report
        .get("providers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let live = rows
        .iter()
        .filter(|row| text(row, "state") == Some("live"))
        .count();
    println!("{live} of {} subscription credentials are live", rows.len());
    for row in rows {
        println!(
            "{:<8} {:<14} {}",
            text(row, "state").unwrap_or_default(),
            text(row, "provider").unwrap_or_default(),
            text(row, "subscription_id").unwrap_or_default()
        );
        if let Some(expires_at) = text(row, "expires_at") {
            println!("    expires_at: {expires_at}");
        }
        if let Some(error) = text(row, "last_redeem_error") {
            println!("    last_redeem_error: {error}");
        }
    }
}

/// What one refresh came to, as lines.
fn print_refresh(verdict: &Value) {
    println!(
        "provider: {}",
        text(verdict, "provider").unwrap_or_default()
    );
    println!(
        "attempted: {}",
        verdict
            .get("attempted")
            .and_then(Value::as_u64)
            .unwrap_or_default()
    );
    println!("result: {}", text(verdict, "result").unwrap_or_default());
    println!("detail: {}", text(verdict, "detail").unwrap_or_default());
}

/// What one sign-in came to, as lines.
fn print_sign_in(verdict: &Value) {
    println!(
        "provider: {}",
        text(verdict, "provider").unwrap_or_default()
    );
    println!(
        "login_item: {}",
        text(verdict, "login_item").unwrap_or_default()
    );
    if let Some(account) = text(verdict, "account").filter(|account| !account.is_empty()) {
        println!("account: {account}");
    }
    println!("result: {}", text(verdict, "result").unwrap_or_default());
    println!("detail: {}", text(verdict, "detail").unwrap_or_default());
}

/// One string field, absent when the report states nothing there. A `null` reads
/// as absent, which is what the pool report writes for an expiry a credential
/// does not state and for a refusal there has not been.
fn text<'a>(report: &'a Value, key: &str) -> Option<&'a str> {
    report.get(key).and_then(Value::as_str)
}
