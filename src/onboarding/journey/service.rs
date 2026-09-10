//! Brama's connection to the Stado journey service: what this product sends
//! it, and what it accepts back as this product's own journey.

use async_trait::async_trait;
use reqwest::Url;
use serde_json::{json, Value};
use uuid::Uuid;
use wisent_onboarding_client::{
    ExperimentAssignment, ExperimentAssignmentRequest, JourneyBundle, JourneyError, RuntimeEvent,
    Transport,
};

use super::super::{FIRST_SUCCESS_FACT, JOURNEY_ID, JOURNEY_VERSION, PRODUCT_ID, STADO_CLIENT};

#[derive(Clone)]
pub(crate) struct BramaTransport {
    remote: Option<RemoteStado>,
}

#[derive(Clone)]
struct RemoteStado {
    base_url: Url,
    token: String,
    client: reqwest::Client,
}

impl BramaTransport {
    pub(crate) fn from_env() -> Self {
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
