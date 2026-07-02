use clap::{Parser, Subcommand};
use tracing::info;

use model_router::subscription_dispatch::{
    collect_subscription_checks, collect_task_quality, CollectOptions, TaskQualityOptions,
};
use model_router::{
    build_default_router, detect_compute_resources, select_model_for_resources, start_server,
    Message,
};

#[derive(Parser)]
#[command(name = "model-router", about = "Multi-provider LLM router")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the OpenAI-compatible HTTP server
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
    },
    /// Run a test inference through the router
    Test {
        /// Model to test with
        #[arg(short, long, default_value = "gpt-4o-mini")]
        model: String,
    },
    /// Detect local hardware capabilities
    Detect,
    /// Collect native CLI subscription/auth checks into subscription-router
    CollectSubscriptionChecks {
        /// Model-router agent/client id whose runtime credentials should be checked
        #[arg(long)]
        agent_id: String,
        /// Optional provider filter: claude_code, codex, kimi, opencode
        #[arg(long)]
        provider: Option<String>,
        /// Allow provider runtime calls when no cheap status command exists
        #[arg(long, default_value_t = false)]
        deep: bool,
        /// Write results to subscription_router_checks cache table
        #[arg(long, default_value_t = false)]
        persist: bool,
    },
    /// Collect deterministic task-quality checks for active subscription models
    CollectTaskQuality {
        /// Model-router agent/client id whose runtime credentials should be checked
        #[arg(long)]
        agent_id: String,
        /// Task key used later as model="task:<task>"
        #[arg(long)]
        task: String,
        /// Prompt sent to each active subscription model
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
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { port } => {
            let router = build_default_router();
            info!("Starting server on port {port}");
            if let Err(e) = start_server(router, port).await {
                eprintln!("Server error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Test { model } => {
            let router = build_default_router();
            let messages = vec![Message {
                role: "user".into(),
                content: "Say hello in one sentence.".into(),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            }];
            let resp = router
                .complete(messages, &model, 256, 0.7, None, None)
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
        Commands::CollectSubscriptionChecks {
            agent_id,
            provider,
            deep,
            persist,
        } => {
            match collect_subscription_checks(CollectOptions {
                agent_id,
                provider,
                deep,
                persist,
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
        Commands::CollectTaskQuality {
            agent_id,
            task,
            prompt,
            expected_exact,
            expected_contains,
            persist,
        } => {
            match collect_task_quality(TaskQualityOptions {
                agent_id,
                task,
                prompt,
                expected_exact,
                expected_contains,
                persist,
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
