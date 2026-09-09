//! Brama's first-use walkthrough: the flow itself lives here, and the four
//! concerns it leans on -- the journey service, the step it writes out, the
//! subject it is about, and where progress is recorded -- live beside it.

mod journey_service;
mod progress_store;
mod step_display;
mod subject;

use std::collections::BTreeMap;

use serde_json::Value;
use uuid::Uuid;
use wisent_onboarding_client::{
    bundle_from_canonical, FileStorage, JourneyClient, JourneyError, ProgressStatus, ScopeKind,
    Transport,
};

use crate::providers::adapter::provider_id_from_route;
use crate::subscription_dispatch::{
    dispatch_direct, dispatch_subscription_for_agent, is_subscription_model,
};
use crate::{Message, ModelRequest};

use journey_service::BramaTransport;
use progress_store::state_path;
use step_display::render_current_step;
use subject::stable_subject_hash;

const PRODUCT_ID: &str = "brama";
const JOURNEY_ID: &str = "first-use";
const JOURNEY_VERSION: &str = "2026-09-05.1";
const FIRST_SUCCESS_FACT: &str = "model_response_received";
const JOURNEY_VERSION_ID: &str = "6d0e4e20-e8cc-4cec-87b8-1556a6167855";
const STADO_CLIENT: &str = "brama";
const STATE_REVISION: &str = "cli:first-use:2026-09-05.1";

/// The journey document compiled into this build, used as the definition the
/// journey client starts from before the journey service states its own.
const BUILT_IN_DEFINITION: &str = include_str!("../onboarding_first_use.json");

/// Run Brama's first-use journey. `reset` discards recorded progress through the
/// journey client -- emitting `onboarding_reset` rather than deleting the state
/// file behind the client's back -- and shows the walkthrough again from its
/// first step in this same invocation.
pub async fn run_first_use(
    model: String,
    agent_id: String,
    allow_provider_cost: bool,
    reset: bool,
) -> Result<bool, JourneyError> {
    let built_in_definition = bundle_from_canonical(
        BUILT_IN_DEFINITION,
        Uuid::parse_str(JOURNEY_VERSION_ID)
            .map_err(|_| JourneyError::Invalid("Brama journey version id".into()))?,
    )?;
    let subject_hash = stable_subject_hash(&agent_id);
    let transport = BramaTransport::from_env();
    let storage = FileStorage::new(state_path());
    let mut journey = JourneyClient::new(
        PRODUCT_ID,
        JOURNEY_ID,
        subject_hash.clone(),
        ScopeKind::Workload,
        transport.clone(),
        storage,
        built_in_definition,
    )?;

    journey.start(STATE_REVISION).await?;
    if reset {
        journey.reset(STATE_REVISION).await?;
        println!(
            "Brama first-use journey reset: recorded progress discarded, showing it again now."
        );
        println!();
    }
    if let Some(progress) = journey.progress() {
        let _ = transport
            .read_state(PRODUCT_ID, progress.attempt_id, &subject_hash)
            .await;
        if progress.status == ProgressStatus::Completed {
            println!(
                "Brama first-use journey is already complete: a real model response was received."
            );
            journey.flush().await?;
            return Ok(true);
        }
    }

    let no_evidence = BTreeMap::new();
    loop {
        journey.expose(STATE_REVISION).await?;
        render_current_step(&journey)?;
        let is_terminal = journey
            .bundle()
            .and_then(|bundle| {
                journey.progress().and_then(|progress| {
                    bundle
                        .definition
                        .screens
                        .iter()
                        .find(|screen| screen.screen_id == progress.current_screen_id)
                })
            })
            .is_some_and(|screen| screen.transitions.is_empty());
        if is_terminal {
            break;
        }
        if journey
            .advance(&no_evidence, STATE_REVISION)
            .await?
            .is_none()
        {
            return Err(JourneyError::Invalid(
                "Brama journey cannot advance with current evidence".into(),
            ));
        }
        println!();
    }

    if !allow_provider_cost {
        println!();
        println!("Next: adopt an existing route registry with --adopt-from, configure provider/auth separately if needed, then re-run this command with --allow-provider-cost.");
        println!("No provider request was sent and onboarding remains in progress.");
        journey.flush().await?;
        return Ok(false);
    }

    println!();
    println!(
        "Sending one billable model request through route {model:?} for workload {agent_id:?}..."
    );
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
    let direct_provider = provider_id_from_route(&request.model)
        .is_some_and(crate::gateway::broker::provider_capability_configured);
    let response = if is_subscription_model(&request.model) && !direct_provider {
        dispatch_subscription_for_agent(&agent_id, &request).await
    } else {
        dispatch_direct(&request).await
    };
    if !response.success {
        eprintln!(
            "Model request failed: {}",
            response.error.unwrap_or_default()
        );
        eprintln!("Onboarding remains in progress because no model response was received.");
        journey.flush().await?;
        return Ok(false);
    }

    println!("Model: {}", response.model);
    println!("Response: {}", response.content);
    println!(
        "Tokens: {} in / {} out",
        response.input_tokens, response.output_tokens
    );
    let mut evidence = BTreeMap::new();
    evidence.insert(FIRST_SUCCESS_FACT.into(), Value::Bool(true));
    let completed = journey.complete(&evidence, STATE_REVISION).await?;
    journey.flush().await?;
    if completed {
        println!("First-use complete: Brama observed model_response_received from the real response above.");
    }
    Ok(completed)
}
