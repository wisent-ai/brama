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

/// One declared connection variable: its text, none when it is not set, or
/// why it cannot be read.
fn declared(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{name} is not Unicode")),
    }
}

impl BramaTransport {
    /// The Stado connection the environment declares. Declaring neither
    /// variable runs the journey locally; declaring only one, or one that
    /// cannot be read or is refused, is said, since it disables the remote
    /// journey the operator meant to enable.
    pub(crate) fn from_env() -> Self {
        let declaration = declared("STADO_INTEGRATION_API_URL")
            .and_then(|url| Ok((url, declared("BRAMA_STADO_INTEGRATION_TOKEN")?)));
        let remote = match declaration {
            Ok((None, None)) => None,
            Ok((Some(base_url), Some(token))) => match RemoteStado::new(&base_url, token) {
                Ok(remote) => Some(remote),
                Err(error) => {
                    tracing::warn!(%error, "onboarding journey: the declared Stado connection is refused");
                    None
                }
            },
            Ok((Some(_), None)) => {
                tracing::warn!("onboarding journey: STADO_INTEGRATION_API_URL is declared without BRAMA_STADO_INTEGRATION_TOKEN, so no Stado connection is made");
                None
            }
            Ok((None, Some(_))) => {
                tracing::warn!("onboarding journey: BRAMA_STADO_INTEGRATION_TOKEN is declared without STADO_INTEGRATION_API_URL, so no Stado connection is made");
                None
            }
            Err(error) => {
                tracing::warn!(%error, "onboarding journey: the declared Stado connection cannot be read");
                None
            }
        };
        Self { remote }
    }
}

impl RemoteStado {
    fn new(base_url: &str, token: String) -> Result<Self, JourneyError> {
        let base_url = Url::parse(base_url).map_err(|error| {
            JourneyError::Invalid(format!("Stado base URL {base_url:?}: {error}"))
        })?;
        let refused = if base_url.scheme() != "https" {
            Some("is not https")
        } else if base_url.host_str().is_none() {
            Some("names no host")
        } else if !base_url.username().is_empty() || base_url.password().is_some() {
            Some("carries credentials")
        } else if base_url.query().is_some() || base_url.fragment().is_some() {
            Some("carries a query or fragment")
        } else if base_url.path() != "/" {
            Some("has a path other than /")
        } else {
            None
        };
        if let Some(refused) = refused {
            return Err(JourneyError::Invalid(format!("Stado base URL {refused}")));
        }
        if token.trim().is_empty() {
            return Err(JourneyError::Invalid(
                "BRAMA_STADO_INTEGRATION_TOKEN is empty".into(),
            ));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| transport("building the client", error))?;
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
            .map_err(|error| transport(operation, error))?;
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .await
            .map_err(|error| transport(operation, error.without_url()))?;
        if !response.status().is_success() {
            return Err(transport(
                operation,
                format!("Stado answered HTTP {}", response.status()),
            ));
        }
        let envelope: Value = response
            .json()
            .await
            .map_err(|error| transport(operation, error.without_url()))?;
        if envelope.get("ok") != Some(&Value::Bool(true)) {
            return Err(transport(operation, format!("Stado refused: {envelope}")));
        }
        envelope
            .get("result")
            .cloned()
            .ok_or_else(|| transport(operation, format!("Stado answered no result: {envelope}")))
    }
}

#[async_trait]
impl Transport for BramaTransport {
    async fn read_bundle(
        &self,
        product_id: &str,
        journey_id: &str,
    ) -> Result<JourneyBundle, JourneyError> {
        let remote = self
            .remote
            .as_ref()
            .ok_or_else(|| unconfigured("bundle.read"))?;
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
            serde_json::from_value(result).map_err(|error| transport("bundle.read", error))?;
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
        let remote = self
            .remote
            .as_ref()
            .ok_or_else(|| unconfigured("events.collect"))?;
        let event =
            serde_json::to_value(event).map_err(|error| transport("events.collect", error))?;
        remote.post("events.collect", &event).await?;
        Ok(())
    }

    async fn read_state(
        &self,
        product_id: &str,
        attempt_id: Uuid,
        subject_hash: &str,
    ) -> Result<Option<Value>, JourneyError> {
        let remote = self
            .remote
            .as_ref()
            .ok_or_else(|| unconfigured("state.read"))?;
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
        let remote = self
            .remote
            .as_ref()
            .ok_or_else(|| unconfigured("experiments.assign"))?;
        let request = serde_json::to_value(request)
            .map_err(|error| transport("experiments.assign", error))?;
        let assignment = remote.post("experiments.assign", &request).await?;
        serde_json::from_value(assignment).map_err(|error| transport("experiments.assign", error))
    }
}

/// A transport refusal with its operation and cause, logged as well so a
/// journey that never reaches Stado leaves a trace in the gateway log.
fn transport(operation: &str, cause: impl std::fmt::Display) -> JourneyError {
    tracing::warn!(%operation, %cause, "onboarding journey transport failed");
    JourneyError::Transport(format!("{operation}: {cause}"))
}

/// No Stado onboarding route is configured, so `operation` was not attempted.
fn unconfigured(operation: &str) -> JourneyError {
    transport(operation, "no Stado onboarding route is configured")
}
