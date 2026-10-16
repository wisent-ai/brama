//! Brama's connection to the Stado journey service: what this product sends
//! it, and what it accepts back as this product's own journey.

use async_trait::async_trait;
use reqwest::Url;
use serde_json::{json, Value};
use uuid::Uuid;
use wisent_onboarding_client::{
    validate_bundle, ExperimentAssignment, ExperimentAssignmentRequest, JourneyBundle,
    JourneyError, RuntimeEvent, Transport,
};

use super::super::{FIRST_SUCCESS_FACT, JOURNEY_ID, JOURNEY_VERSION, PRODUCT_ID, STADO_CLIENT};

#[derive(Clone)]
pub(crate) struct BramaTransport {
    /// The Stado connection, or the sentence saying why there is none: every
    /// operation is refused with that sentence, so a connection that was
    /// declared and refused is never reported as one never configured.
    remote: Result<RemoteStado, String>,
}

#[derive(Clone)]
struct RemoteStado {
    base_url: Url,
    token: String,
    client: reqwest::Client,
}

const URL_VARIABLE: &str = "STADO_INTEGRATION_API_URL";
const TOKEN_VARIABLE: &str = "BRAMA_STADO_INTEGRATION_TOKEN";

impl BramaTransport {
    /// The Stado connection the environment declares. Declaring neither
    /// variable runs the journey locally; declaring only one, or one that
    /// cannot be read or is refused, is the refusal every remote operation
    /// answers with, since it disables the remote journey the operator meant
    /// to enable.
    pub(crate) fn from_env() -> Self {
        let declaration = crate::environment::declared_text(URL_VARIABLE)
            .and_then(|url| Ok((url, crate::environment::declared_text(TOKEN_VARIABLE)?)));
        let remote = match declaration {
            Ok((None, None)) => Err(format!(
                "no Stado onboarding route is configured: neither {URL_VARIABLE} nor {TOKEN_VARIABLE} is declared"
            )),
            Ok((Some(base_url), Some(token))) => RemoteStado::new(&base_url, token)
                .map_err(|error| format!("the declared Stado onboarding connection is refused: {error}")),
            Ok((Some(_), None)) => Err(format!(
                "{URL_VARIABLE} is declared without {TOKEN_VARIABLE}, so no Stado connection is made"
            )),
            Ok((None, Some(_))) => Err(format!(
                "{TOKEN_VARIABLE} is declared without {URL_VARIABLE}, so no Stado connection is made"
            )),
            Err(error) => Err(format!(
                "the declared Stado onboarding connection cannot be read: {error}"
            )),
        };
        Self { remote }
    }

    /// The Stado connection `operation` is sent through, or its refusal.
    fn remote(&self, operation: &str) -> Result<&RemoteStado, JourneyError> {
        self.remote
            .as_ref()
            .map_err(|cause| transport(operation, cause))
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
        let response = stado_wait::until(
            stado_wait::Kind::Network,
            format!("onboarding {operation}"),
            "Stado",
            self.client
                .post(endpoint)
                .bearer_auth(&self.token)
                .json(body)
                .send(),
        )
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
        let remote = self.remote("bundle.read")?;
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
        // The client's own envelope rules (its schema version, this product
        // and journey, the canonical definition and its digest), then the
        // version and first-success fact this product was built for.
        let bundle = validate_bundle(bundle, PRODUCT_ID, JOURNEY_ID)?;
        if bundle.definition.journey_version != JOURNEY_VERSION
            || bundle.definition.first_success_fact != FIRST_SUCCESS_FACT
        {
            return Err(JourneyError::Invalid("Brama journey identity".into()));
        }
        Ok(bundle)
    }

    async fn collect_event(&self, event: &RuntimeEvent) -> Result<(), JourneyError> {
        let remote = self.remote("events.collect")?;
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
        let remote = self.remote("state.read")?;
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
        let remote = self.remote("experiments.assign")?;
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
