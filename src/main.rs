use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

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
        /// Existing Brama inference-routes JSON file to review during first use
        #[arg(long, value_name = "FILE")]
        adopt_from: Option<PathBuf>,
        /// Destination route registry for --adopt-from
        #[arg(long, value_name = "FILE")]
        adopt_into: Option<PathBuf>,
        /// Exact source alias to adopt; repeat for more than one
        #[arg(long = "adopt-select", value_name = "ALIAS")]
        adopt_selected_aliases: Vec<String>,
        /// Adopt every importable or already unchanged alias from --adopt-from
        #[arg(long, default_value_t = false)]
        adopt_all_importable: bool,
        /// Persist the reviewed --adopt-from selection
        #[arg(long, default_value_t = false)]
        adopt_apply: bool,
        /// Replace conflicting aliases during --adopt-apply
        #[arg(long, default_value_t = false)]
        adopt_replace_conflicts: bool,
    },
    /// Review and adopt an existing Brama inference-route registry
    Adopt {
        /// Existing Brama inference-routes JSON file to review
        #[arg(long, value_name = "FILE")]
        from: PathBuf,
        /// Destination route registry; defaults to BRAMA_INFERENCE_ROUTES_FILE or ~/.config/brama/inference-routes.json
        #[arg(long, value_name = "FILE")]
        into: Option<PathBuf>,
        /// Agent whose Skarbiec subscription identities should be discovered
        #[arg(long, default_value = "wisent-app")]
        agent_id: String,
        /// Persist the selected aliases after showing the same review data
        #[arg(long, default_value_t = false)]
        apply: bool,
        /// Exact source alias to persist; repeat for more than one
        #[arg(long = "select", value_name = "ALIAS")]
        selected_aliases: Vec<String>,
        /// Select every importable or already unchanged alias
        #[arg(long, default_value_t = false)]
        all_importable: bool,
        /// Replace conflicting aliases; deployment conflicts are never replaced
        #[arg(long, default_value_t = false)]
        replace_conflicts: bool,
        /// Print the preview or result as JSON
        #[arg(long, default_value_t = false)]
        json: bool,
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
            adopt_from,
            adopt_into,
            adopt_selected_aliases,
            adopt_all_importable,
            adopt_apply,
            adopt_replace_conflicts,
        } => {
            if let Some(from) = adopt_from.as_deref() {
                match run_adoption(AdoptionOptions {
                    from,
                    into: adopt_into.as_deref(),
                    agent_id: &agent_id,
                    apply: adopt_apply,
                    selected_aliases: &adopt_selected_aliases,
                    all_importable: adopt_all_importable,
                    replace_conflicts: adopt_replace_conflicts,
                    json: false,
                })
                .await
                {
                    Ok(false) => {
                        println!(
                            "Review complete. Re-run with --adopt-apply and --adopt-select <ALIAS>, or --adopt-all-importable, to persist a selection."
                        );
                        return;
                    }
                    Ok(true) => {}
                    Err(error) => {
                        eprintln!("Configuration adoption error: {error}");
                        std::process::exit(1);
                    }
                }
            } else if adopt_apply
                || adopt_all_importable
                || adopt_replace_conflicts
                || !adopt_selected_aliases.is_empty()
                || adopt_into.is_some()
            {
                eprintln!("Configuration adoption error: --adopt-from is required for every adoption option");
                std::process::exit(1);
            }

            match brama::onboarding::run_first_use(model, agent_id, allow_provider_cost, reset)
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
            }
        }
        Commands::Adopt {
            from,
            into,
            agent_id,
            apply,
            selected_aliases,
            all_importable,
            replace_conflicts,
            json,
        } => {
            if let Err(error) = run_adoption(AdoptionOptions {
                from: &from,
                into: into.as_deref(),
                agent_id: &agent_id,
                apply,
                selected_aliases: &selected_aliases,
                all_importable,
                replace_conflicts,
                json,
            })
            .await
            {
                eprintln!("Configuration adoption error: {error}");
                std::process::exit(1);
            }
        }
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

struct AdoptionOptions<'a> {
    from: &'a Path,
    into: Option<&'a Path>,
    agent_id: &'a str,
    apply: bool,
    selected_aliases: &'a [String],
    all_importable: bool,
    replace_conflicts: bool,
    json: bool,
}

async fn run_adoption(options: AdoptionOptions<'_>) -> Result<bool, String> {
    let AdoptionOptions {
        from,
        into,
        agent_id,
        apply,
        selected_aliases,
        all_importable,
        replace_conflicts,
        json,
    } = options;
    if !apply && (all_importable || replace_conflicts || !selected_aliases.is_empty()) {
        return Err(
            "--apply is required with --select, --all-importable, or --replace-conflicts"
                .to_string(),
        );
    }
    if apply && !all_importable && selected_aliases.is_empty() {
        return Err(
            "--apply requires at least one --select <ALIAS> or --all-importable".to_string(),
        );
    }
    if selected_aliases.iter().collect::<HashSet<_>>().len() != selected_aliases.len() {
        return Err("selected aliases must be unique".to_string());
    }
    let document = read_adoption_source(from)?;
    let destination = match into {
        Some(path) => path.to_path_buf(),
        None => brama::config_adoption::default_destination()?,
    };
    let source_name = from.display().to_string();
    let preview =
        brama::config_adoption::preview_document(&document, &source_name, &destination, agent_id)
            .await?;
    if !apply {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&preview)
                    .map_err(|error| format!("cannot encode adoption preview: {error}"))?
            );
        } else {
            print_adoption_preview(&preview);
        }
        return Ok(false);
    }

    let mut selection = selected_aliases.to_vec();
    if all_importable {
        selection.extend(
            preview
                .candidates
                .iter()
                .filter(|candidate| {
                    matches!(
                        candidate.disposition,
                        brama::config_adoption::AdoptionDisposition::Importable
                            | brama::config_adoption::AdoptionDisposition::Unchanged
                    )
                })
                .map(|candidate| candidate.alias.clone()),
        );
    }
    selection.sort();
    selection.dedup();
    let result = brama::config_adoption::apply_document(
        &document,
        &source_name,
        &destination,
        agent_id,
        &selection,
        replace_conflicts,
    )
    .await?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result)
                .map_err(|error| format!("cannot encode adoption result: {error}"))?
        );
    } else {
        print_adoption_result(&result);
    }
    Ok(true)
}

fn read_adoption_source(path: &Path) -> Result<String, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "{} must be a regular non-symlink file",
            path.display()
        ));
    }
    if metadata.len() > 1024 * 1024 {
        return Err(format!(
            "{} exceeds the 1048576-byte configuration limit",
            path.display()
        ));
    }
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {} as UTF-8: {error}", path.display()))
}

fn print_adoption_preview(preview: &brama::config_adoption::AdoptionPreview) {
    println!("Source: {}", preview.source);
    println!("Destination: {}", preview.destination);
    println!("Agent: {}", preview.agent_id);
    println!(
        "Configured provider acquisitions: {}",
        preview.providers.len()
    );
    for provider in &preview.providers {
        println!("  {} ({})", provider.provider, provider.acquisition);
    }
    println!(
        "Skarbiec subscriptions: {} ({})",
        preview.subscriptions.len(),
        preview.subscription_discovery
    );
    for subscription in &preview.subscriptions {
        println!(
            "  {} / {} ({})",
            subscription.provider, subscription.subscription_id, subscription.status
        );
    }
    println!("Route candidates: {}", preview.candidates.len());
    for candidate in &preview.candidates {
        let chain = std::iter::once(candidate.primary.as_str())
            .chain(candidate.fallbacks.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" -> ");
        println!(
            "  {:<11} {} = {}",
            adoption_disposition(candidate.disposition),
            candidate.alias,
            chain
        );
        println!("      {}", candidate.detail);
        if let Some(primary) = candidate.existing_primary.as_deref() {
            let current = std::iter::once(primary)
                .chain(candidate.existing_fallbacks.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" -> ");
            println!("      current: {current}");
        }
        if !candidate.deployments.is_empty() {
            println!("      deployments: {}", candidate.deployments.join(", "));
        }
    }
    for deployment in &preview.unreferenced_deployments {
        println!("  rejected    deployment {deployment}: no source alias references it");
    }
}

fn print_adoption_result(result: &brama::config_adoption::AdoptionResult) {
    println!("Destination: {}", result.destination);
    println!(
        "Imported: {}; unchanged: {}; conflicting: {}; rejected: {}",
        result.imported, result.unchanged, result.conflicting, result.rejected
    );
    for item in &result.items {
        println!(
            "  {:<11} {}: {}",
            adoption_disposition(item.disposition),
            item.alias,
            item.detail
        );
    }
}

fn adoption_disposition(disposition: brama::config_adoption::AdoptionDisposition) -> &'static str {
    use brama::config_adoption::AdoptionDisposition;
    match disposition {
        AdoptionDisposition::Importable => "importable",
        AdoptionDisposition::Imported => "imported",
        AdoptionDisposition::Unchanged => "unchanged",
        AdoptionDisposition::Conflicting => "conflicting",
        AdoptionDisposition::Rejected => "rejected",
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
