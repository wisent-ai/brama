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

use crate::{Message, ModelRequest};
use crate::providers::adapter::provider_id_from_route;
use crate::subscription_dispatch::{
    dispatch_direct_with_fallback, dispatch_subscription_for_agent, is_subscription_model,
};

const PRODUCT_ID: &str = "brama";
const JOURNEY_ID: &str = "first-use";
const JOURNEY_VERSION: &str = "2026-08-04.1";
const FIRST_SUCCESS_FACT: &str = "model_response_received";
const JOURNEY_VERSION_ID: &str = "5a4a397b-4839-4d1e-b90f-31c543a6ebc9";
const STADO_CLIENT: &str = "brama";
const STATE_REVISION: &str = "cli:first-use:2026-08-04.1";

const FALLBACK_DEFINITION: &str = r#"{"schema_version":1,"product_id":"brama","journey_id":"first-use","journey_version":"2026-08-04.1","entry_screen_id":"provider-neutral-routing","first_success_fact":"model_response_received","published_at":"2026-08-04T00:00:00Z","source_revision":"brama-first-use-2026-08-04.1","screens":[{"screen_id":"provider-neutral-routing","screen_kind":"explanation","title_key":"onboarding.routing.title","body_key":"onboarding.routing.body","required":true,"actions":["next"],"transitions":[{"next_screen_id":"request-response-contract","reason_code":"routing_understood","priority":0}],"presentation":{"title":"Route once, independent of provider","body":"Brama accepts one routing request and selects the configured provider/model route behind it. Your application keeps the same message contract while routes, aliases, and subscription-backed selection can change independently. Provider credentials and caller authentication are provisioned separately; onboarding never creates or changes them."}},{"screen_id":"request-response-contract","screen_kind":"explanation","title_key":"onboarding.contract.title","body_key":"onboarding.contract.body","required":true,"actions":["next"],"transitions":[{"next_screen_id":"real-model-response","reason_code":"contract_understood","priority":0}],"presentation":{"title":"Use the OpenAI-compatible request and response contract","body":"Send model, messages, max_tokens, and temperature to POST /v1/chat/completions. A successful response returns id, model, choices[0].message, finish_reason, and token usage; routing details stay Brama-owned.","request_example":"{\"model\":\"openai/default\",\"messages\":[{\"role\":\"user\",\"content\":\"Say hello in one sentence.\"}],\"max_tokens\":256,\"temperature\":0.7}","response_example":"{\"id\":\"chatcmpl-...\",\"model\":\"...\",\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"...\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":0,\"completion_tokens\":0,\"total_tokens\":0}}"}},{"screen_id":"real-model-response","screen_kind":"first_success","title_key":"onboarding.response.title","body_key":"onboarding.response.body","required":true,"completion_evidence":{"kind":"fact","fact":"model_response_received","operator":"eq","value":true},"actions":["send_real_model_request"],"transitions":[],"presentation":{"title":"Receive one real model response","body":"Run the onboarding request through your configured route. Completion is recorded only after Brama receives a successful response from a real model; viewing this step or allowing provider cost is not sufficient. Auth and provider setup remain separate."}}],"analytics_contract":{"contract_version":"1","surface":"cli","exposure_event":"onboarding_step_viewed","primary_action_event":"onboarding_step_completed","completion_event":"onboarding_completed","first_success_event":"onboarding_first_success_observed"}}"#;

#[derive(Clone)]
struct BramaTransport {
    remote: Option<RemoteStado>,
}

#[derive(Clone)]
struct RemoteStado {
    base_url: Url,
    token: String,
    client: reqwest::Client,
}

impl BramaTransport {
    fn from_env() -> Self {
        let remote = std::env::var("STADO_INTEGRATION_API_URL")
            .ok()
            .zip(std::env::var("BRAMA_STADO_INTEGRATION_TOKEN").ok())
            .and_then(|(base_url, token)| RemoteStado::new(&base_url, token).ok());
        Self { remote }
    }
}

impl RemoteStado {
    fn new(base_url: &str, token: String) -> Result<Self, JourneyError> {
        let base_url =
            Url::parse(base_url).map_err(|_| JourneyError::Invalid("Stado base URL".into()))?;
        if base_url.scheme() != "https"
            || base_url.host_str().is_none()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || base_url.path() != "/"
            || token.trim().is_empty()
        {
            return Err(JourneyError::Invalid("Stado configuration".into()));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| JourneyError::Transport)?;
        Ok(Self {
            base_url,
            token,
            client,
        })
    }

    async fn post(&self, operation: &str, body: &Value) -> Result<Value, JourneyError> {
        let endpoint = self
            .base_url
            .join(&format!(
                "integration/{STADO_CLIENT}/onboarding/{PRODUCT_ID}/{operation}"
            ))
            .map_err(|_| JourneyError::Transport)?;
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .map_err(|_| JourneyError::Transport)?;
        if !response.status().is_success() {
            return Err(JourneyError::Transport);
        }
        let envelope: Value = response.json().await.map_err(|_| JourneyError::Transport)?;
        if envelope.get("ok") != Some(&Value::Bool(true)) {
            return Err(JourneyError::Transport);
        }
        envelope
            .get("result")
            .cloned()
            .ok_or(JourneyError::Transport)
    }
}

#[async_trait]
impl Transport for BramaTransport {
    async fn read_bundle(
        &self,
        product_id: &str,
        journey_id: &str,
    ) -> Result<JourneyBundle, JourneyError> {
        let remote = self.remote.as_ref().ok_or(JourneyError::Transport)?;
        let result = remote
            .post(
                "bundle.read",
                &json!({
                    "product_id": product_id,
                    "journey_id": journey_id,
                    "journey_version": JOURNEY_VERSION,
                    "if_none_match": null
                }),
            )
            .await?;
        let bundle: JourneyBundle =
            serde_json::from_value(result).map_err(|_| JourneyError::Transport)?;
        if bundle.definition.schema_version != 1
            || bundle.definition.product_id != PRODUCT_ID
            || bundle.definition.journey_id != JOURNEY_ID
            || bundle.definition.journey_version != JOURNEY_VERSION
            || bundle.definition.first_success_fact != FIRST_SUCCESS_FACT
        {
            return Err(JourneyError::Invalid("Brama journey identity".into()));
        }
        Ok(bundle)
    }

    async fn collect_event(&self, event: &RuntimeEvent) -> Result<(), JourneyError> {
        let remote = self.remote.as_ref().ok_or(JourneyError::Transport)?;
        let event = serde_json::to_value(event).map_err(|_| JourneyError::Transport)?;
        remote.post("events.collect", &event).await?;
        Ok(())
    }

    async fn read_state(
        &self,
        product_id: &str,
        attempt_id: Uuid,
        subject_hash: &str,
    ) -> Result<Option<Value>, JourneyError> {
        let remote = self.remote.as_ref().ok_or(JourneyError::Transport)?;
        let state = remote
            .post(
                "state.read",
                &json!({
                    "product_id": product_id,
                    "attempt_id": attempt_id,
                    "subject_hash": subject_hash
                }),
            )
            .await?;
        Ok((state.get("found") != Some(&Value::Bool(false))).then_some(state))
    }

    async fn assign_experiment(
        &self,
        request: &ExperimentAssignmentRequest,
    ) -> Result<ExperimentAssignment, JourneyError> {
        let remote = self.remote.as_ref().ok_or(JourneyError::Transport)?;
        let request = serde_json::to_value(request).map_err(|_| JourneyError::Transport)?;
        let assignment = remote.post("experiments.assign", &request).await?;
        serde_json::from_value(assignment).map_err(|_| JourneyError::Transport)
    }
}

pub async fn run_first_use(
    model: String,
    agent_id: String,
    allow_provider_cost: bool,
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
        println!("Next: configure provider/auth separately if needed, then re-run this command with --allow-provider-cost.");
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

fn render_current_step<T: Transport, S: wisent_onboarding_client::Storage>(
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

fn stable_subject_hash(agent_id: &str) -> String {
    let digest = Sha256::digest(format!("{PRODUCT_ID}:workload:{agent_id}").as_bytes());
    hex::encode(digest)
}

fn state_path() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(path).join("brama/onboarding.json");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/state/brama/onboarding.json");
    }
    std::env::temp_dir().join("brama/onboarding.json")
}
