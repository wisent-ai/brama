mod cli;

use clap::{Parser, Subcommand};

use cli::adoption::AdoptArgs;
use cli::aliases::{AliasesArgs, RoutesCommand};
use cli::catalogue::{CatalogueCommand, ModelsArgs};
use cli::decisions::DecideArgs;
use cli::diagnostics::{CollectTaskQualityArgs, TestArgs};
use cli::launcher::LauncherCommand;
use cli::maintain::MaintainArgs;
use cli::media::{ImageArgs, MusicArgs, SpeakArgs, VideoCommand, VoicesCommand};
use cli::onboarding::OnboardArgs;
use cli::probe::ProbeArgs;
use cli::review::ReviewArgs;
use cli::serving::ServeArgs;
use cli::stub::StubArgs;
use cli::subscriptions::credentials::SubscriptionCommand;
use cli::subscriptions::SubscriptionsArgs;
use cli::version_gate::VersionGateCommand;
use cli::workload::WorkloadCommand;

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
    /// Answer typed questions through a decision alias
    Decide(DecideArgs),
    /// Review a change through a gateway with read and search tools confined
    /// to one directory; exits 0 on approve, 3 on request changes, 1 on failure
    Review(ReviewArgs),
    /// Report the subscription pool this gateway routes over
    Subscriptions(SubscriptionsArgs),
    /// Report every model alias this gateway declares and whether it can serve
    Aliases(AliasesArgs),
    /// List the models this gateway can name, with kind, weights and categories
    Models(ModelsArgs),
    /// Act on the model categories this gateway declares
    Categories {
        #[command(subcommand)]
        command: CatalogueCommand,
    },
    /// Generate one image through an image route
    Image(ImageArgs),
    /// Start or read back one video job
    Video {
        #[command(subcommand)]
        command: VideoCommand,
    },
    /// Speak one text through a voice route
    Speak(SpeakArgs),
    /// Compose one song through a music route
    Music(MusicArgs),
    /// List or clone voices on a voice route's account
    Voices {
        #[command(subcommand)]
        command: VoicesCommand,
    },
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
    /// Act on this installation's workload identity in the vault
    Workload {
        #[command(subcommand)]
        command: WorkloadCommand,
    },
    /// Steps of `start-with-skarbiec` before the gateway starts
    #[command(hide = true)]
    Launcher {
        #[command(subcommand)]
        command: LauncherCommand,
    },
    /// Serve a loopback OpenAI-shaped stub provider for the documentation
    /// walk-through (models stub-ok, stub-401, stub-429)
    #[command(hide = true)]
    StubProvider(StubArgs),
    /// Ask the running gateway on this host to serve one real request per
    /// alias and report the answer or refusal
    Probe(ProbeArgs),
    /// Explain, on this host, why the gateway is or is not serving: units,
    /// installed generations, trust registry, service env, grants, alias
    /// routes, reachability and the current boot attempt. Read-only.
    Diagnose,
    /// Run one maintenance pass on the serving gateway: read aged plan usage
    /// reports, renew grants near expiry, take a fresh readiness reading.
    /// Exits 1 naming each failed step.
    Maintain(MaintainArgs),
    /// The pull-request version gate: surface, contract, baseline and the
    /// rule's shared-fixture conformance
    #[command(hide = true)]
    VersionGate {
        #[command(subcommand)]
        command: VersionGateCommand,
    },
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
        Commands::Decide(args) => cli::decisions::decide(args).await,
        Commands::Review(args) => cli::review::run(args).await,
        Commands::Subscriptions(args) => cli::subscriptions::report(args).await,
        Commands::Aliases(args) => cli::aliases::report(args).await,
        Commands::Models(args) => cli::catalogue::models(args).await,
        Commands::Categories { command } => cli::catalogue::run_categories(command).await,
        Commands::Image(args) => cli::media::image(args).await,
        Commands::Video { command } => cli::media::video(command).await,
        Commands::Speak(args) => cli::media::speak(args).await,
        Commands::Music(args) => cli::media::music(args).await,
        Commands::Voices { command } => cli::media::voices(command).await,
        Commands::Routes { command } => cli::aliases::routes(command),
        Commands::Subscription { command } => cli::subscriptions::credentials::run(command).await,
        Commands::CollectTaskQuality(args) => cli::diagnostics::collect_task_quality(args).await,
        Commands::Workload { command } => cli::workload::run(command),
        Commands::Launcher { command } => cli::launcher::run(command),
        Commands::StubProvider(args) => cli::stub::serve(args).await,
        Commands::Probe(args) => cli::probe::run(args).await,
        Commands::Diagnose => cli::diagnose::run().await,
        Commands::Maintain(args) => cli::maintain::run(args).await,
        Commands::VersionGate { command } => {
            cli::version_gate::run(command, <Cli as clap::CommandFactory>::command())
        }
    }
}
