use std::io::Read;

use clap::{Parser, Subcommand};
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
        } => match brama::onboarding::run_first_use(model, agent_id, allow_provider_cost).await {
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
