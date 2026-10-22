//! The contribution ledger as Brama reaches it: where it is, the bearer Brama
//! proves itself with, and the three questions Brama asks — record this
//! contribution, link this identity, does this caller hold this entitlement.

use serde_json::{json, Value};

use super::PRODUCT;

/// Where the ledger answers; undeclared means this gateway has none.
pub(super) const LEDGER_URL_ENV: &str = "BRAMA_CONTRIBUTION_LEDGER_URL";
/// The aliases served only to a caller holding the entitlement to them,
/// comma-separated; undeclared means none is gated.
const GATED_ALIASES_ENV: &str = "BRAMA_ENTITLEMENT_GATED_ALIASES";
/// The Skarbiec resource whose route names the reporter bearer.
const LEDGER_RESOURCE: &str = "contribution-ledger";
/// The entitlement kind an alias is gated by; its scope is the alias.
const MODEL_ACCESS: &str = "model.access";

/// The ledger's address, when this gateway declares one.
pub fn ledger_url() -> Result<Option<String>, String> {
    Ok(crate::environment::declared_setting(LEDGER_URL_ENV)?
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty()))
}

/// Whether `alias` is served only to a caller holding the entitlement to it.
pub fn gated(alias: &str) -> Result<bool, String> {
    Ok(crate::environment::declared_setting(GATED_ALIASES_ENV)?
        .is_some_and(|aliases| aliases.split(',').any(|gated| gated.trim() == alias)))
}

/// Why the ledger gave no answer: none declared, or one that did not answer.
#[derive(Debug)]
pub enum Unanswered {
    Undeclared,
    Failed(String),
}

impl std::fmt::Display for Unanswered {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unanswered::Undeclared => write!(
                formatter,
                "this gateway declares no contribution ledger ({LEDGER_URL_ENV})"
            ),
            Unanswered::Failed(cause) => {
                write!(formatter, "the contribution ledger did not answer: {cause}")
            }
        }
    }
}

/// POST `body` to the ledger's `route` as this product, answering the
/// ledger's status and JSON.
async fn post(route: &str, body: &Value) -> Result<(reqwest::StatusCode, Value), Unanswered> {
    let url = ledger_url()
        .map_err(Unanswered::Failed)?
        .ok_or(Unanswered::Undeclared)?;
    let target = format!("{url}{route}");
    let bearer = super::super::obtain_credential(LEDGER_RESOURCE, None)
        .await
        .map_err(|failure| Unanswered::Failed(format!("its bearer could not be read: {failure}")))?;
    let bearer = bearer
        .expose_utf8()
        .map_err(|error| Unanswered::Failed(format!("its bearer is not text: {error:?}")))?
        .to_string();
    let client = crate::providers::adapter::control_client().map_err(Unanswered::Failed)?;
    let response = stado_wait::until(
        stado_wait::Kind::Network,
        "ask the contribution ledger",
        &target,
        client
            .post(&target)
            .bearer_auth(bearer)
            .header("x-wisent-reporter", PRODUCT)
            .json(body)
            .send(),
    )
    .await
    .map_err(|error| Unanswered::Failed(format!("POST {target}: {error}")))?;
    let status = response.status();
    let text = stado_wait::until(
        stado_wait::Kind::Network,
        "read the contribution ledger's answer",
        &target,
        response.text(),
    )
    .await
    .map_err(|error| Unanswered::Failed(format!("POST {target} answered {status} unreadably: {error}")))?;
    let answer: Value = serde_json::from_str(&text).map_err(|error| {
        Unanswered::Failed(format!("POST {target} answered {status} with no JSON ({error}): {text}"))
    })?;
    Ok((status, answer))
}

/// The ledger's own refusal sentence, or the status and body when it gave none.
fn refusal(status: reqwest::StatusCode, answer: &Value) -> String {
    match answer.pointer("/error/message").and_then(Value::as_str) {
        Some(message) => message.to_string(),
        None => format!("the ledger answered {status}: {answer}"),
    }
}

async fn accepted(route: &str, body: &Value) -> Result<Value, Unanswered> {
    let (status, answer) = post(route, body).await?;
    if status.is_success() {
        Ok(answer)
    } else {
        Err(Unanswered::Failed(refusal(status, &answer)))
    }
}

/// Record one contribution event, idempotent by its `source_event_id`.
pub(crate) async fn report(event: &Value) -> Result<Value, Unanswered> {
    accepted("/api/v1/contributions/events", event).await
}

/// What the ledger answered about one caller and one gated alias.
#[derive(Debug)]
pub enum Gate {
    Granted { entitlement_id: String },
    Denied { reason: String, message: String },
}

/// Whether `contributor_id` holds the entitlement to `alias` now.
pub async fn check_entitlement(contributor_id: &str, alias: &str) -> Result<Gate, Unanswered> {
    let answer = accepted(
        "/api/v1/entitlements/check",
        &json!({"contributor_id": contributor_id, "kind": MODEL_ACCESS, "scope": alias}),
    )
    .await?;
    let text = |key: &str| answer.get(key).and_then(Value::as_str).map(str::to_string);
    match (
        answer.get("granted").and_then(Value::as_bool),
        text("entitlement_id"),
        text("reason"),
        text("message"),
    ) {
        (Some(true), Some(entitlement_id), _, _) => Ok(Gate::Granted { entitlement_id }),
        (Some(false), _, Some(reason), Some(message)) => Ok(Gate::Denied { reason, message }),
        _ => Err(Unanswered::Failed(format!(
            "the ledger's entitlement answer is neither a grant nor a denial with its reason: {answer}"
        ))),
    }
}
