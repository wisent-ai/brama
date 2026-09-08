//! Part of `onboarding`, split out to keep every file inside the line limit.
#![allow(unused_imports)]

use super::*;
use std::collections::BTreeMap;
use std::path::PathBuf;
use async_trait::async_trait;
use reqwest::Url;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use wisent_onboarding_client::{
    bundle_from_canonical, ExperimentAssignment, ExperimentAssignmentRequest, FileStorage,
    JourneyBundle, JourneyClient, JourneyError, ProgressStatus, RuntimeEvent, ScopeKind, Transport,
};
use crate::providers::adapter::provider_id_from_route;
use crate::subscription_dispatch::{
    dispatch_direct_with_fallback, dispatch_subscription_for_agent, is_subscription_model,
};
use crate::{Message, ModelRequest};

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
    let fallback = bundle_from_canonical(
        FALLBACK_DEFINITION,
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
        fallback,
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
        dispatch_direct_with_fallback(&request, &[]).await
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

pub(crate) fn render_current_step<T: Transport, S: wisent_onboarding_client::Storage>(
    journey: &JourneyClient<T, S>,
) -> Result<(), JourneyError> {
    let bundle = journey.bundle().ok_or(JourneyError::NotStarted)?;
    let progress = journey.progress().ok_or(JourneyError::NotStarted)?;
    let screen = bundle
        .definition
        .screens
        .iter()
        .find(|screen| screen.screen_id == progress.current_screen_id)
        .ok_or_else(|| JourneyError::Invalid("Brama current screen".into()))?;
    let title = screen
        .presentation
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(&screen.title_key);
    let body = screen
        .presentation
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or(&screen.body_key);
    println!("{title}");
    println!("{body}");
    for key in ["request_example", "response_example"] {
        if let Some(value) = screen.presentation.get(key).and_then(Value::as_str) {
            println!("{key}: {value}");
        }
    }
    Ok(())
}

pub(crate) fn stable_subject_hash(agent_id: &str) -> String {
    let digest = Sha256::digest(format!("{PRODUCT_ID}:workload:{agent_id}").as_bytes());
    hex::encode(digest)
}

pub(crate) fn state_path() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(path).join("brama/onboarding.json");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/state/brama/onboarding.json");
    }
    std::env::temp_dir().join("brama/onboarding.json")
}
