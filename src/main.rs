mod cli;

use clap::{Parser, Subcommand};

use cli::adoption::AdoptArgs;
use cli::aliases::{AliasesArgs, RoutesCommand};
use cli::diagnostics::{CollectTaskQualityArgs, TestArgs};
use cli::onboarding::OnboardArgs;
use cli::serving::ServeArgs;
use cli::subscriptions::credentials::SubscriptionCommand;
use cli::subscriptions::SubscriptionsArgs;

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
    Serve(ServeArgs),
    /// Follow Brama's first-use journey and optionally receive one real model response
    Onboard(OnboardArgs),
    /// Review and adopt an existing Brama inference-route registry
    Adopt(AdoptArgs),
    /// Run a test inference through the router
    Test(TestArgs),
    /// Detect local hardware capabilities
    Detect,
    /// Serve the read-only stdio MCP server (agent surface)
    Mcp,
    /// Report the subscription pool this gateway routes over
    Subscriptions(SubscriptionsArgs),
    /// Report every model alias this gateway declares and whether it can serve
    Aliases(AliasesArgs),
    /// Act on this gateway's inference-route registry
    Routes {
        #[command(subcommand)]
        command: RoutesCommand,
    },
    /// Act on one provider's subscription credentials
    Subscription {
        #[command(subcommand)]
        command: SubscriptionCommand,
    },
    /// Collect deterministic task-quality checks for active provider routes
    CollectTaskQuality(CollectTaskQualityArgs),
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    match Cli::parse().command {
        Commands::Version => cli::diagnostics::print_version(),
        Commands::Serve(args) => cli::serving::serve(args).await,
        Commands::Onboard(args) => cli::onboarding::onboard(args).await,
        Commands::Adopt(args) => cli::adoption::adopt(args).await,
        Commands::Test(args) => cli::diagnostics::test_inference(args).await,
        Commands::Detect => cli::diagnostics::detect(),
        Commands::Mcp => cli::serving::mcp(),
        Commands::Subscriptions(args) => cli::subscriptions::report(args).await,
        Commands::Aliases(args) => cli::aliases::report(args),
        Commands::Routes { command } => cli::aliases::routes(command),
        Commands::Subscription { command } => cli::subscriptions::credentials::run(command).await,
        Commands::CollectTaskQuality(args) => cli::diagnostics::collect_task_quality(args).await,
    }
}
