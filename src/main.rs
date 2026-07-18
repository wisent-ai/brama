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
    /// Start the OpenAI-compatible HTTP server
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value_t = 8080)]
        port: u16,
    },
    /// Run a test inference through the router
    Test {
        /// Canonical provider/model route to test
        #[arg(short, long, default_value = "openai/gpt-5.4")]
        model: String,
        /// Jeden agent/client id whose provider credential should be used
        #[arg(long, default_value = "wisent-app")]
        agent_id: String,
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
    },
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Serve { port } => {
            info!("Starting server on port {port}");
            if let Err(e) = start_server(port).await {
                eprintln!("Server error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Test { model, agent_id } => {
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
                subscription_decision_id: None,
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
