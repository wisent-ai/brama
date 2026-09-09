//! `brama onboard`: the first-use walkthrough, with the optional registry
//! review the walkthrough offers before any provider request.

use std::path::PathBuf;

use clap::Args;

use super::adoption::{run_adoption, AdoptionSelection};

#[derive(Args)]
pub(crate) struct OnboardArgs {
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
}

pub(crate) async fn onboard(args: OnboardArgs) {
    let OnboardArgs {
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
    } = args;
    if let Some(from) = adopt_from.as_deref() {
        match run_adoption(
            from,
            adopt_into.as_deref(),
            &agent_id,
            AdoptionSelection {
                apply: adopt_apply,
                selected_aliases: &adopt_selected_aliases,
                all_importable: adopt_all_importable,
                replace_conflicts: adopt_replace_conflicts,
            },
            false,
        )
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
        eprintln!(
            "Configuration adoption error: --adopt-from is required for every adoption option"
        );
        std::process::exit(1);
    }

    match brama::onboarding::run_first_use(model, agent_id, allow_provider_cost, reset).await {
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
