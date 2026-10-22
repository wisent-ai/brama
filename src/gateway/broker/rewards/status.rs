//! One report of this gateway's side of the contribution ledger, answered the
//! same to `brama rewards status` and to the console's `GET /v1/rewards`: the
//! ledger it declares, whether an alias is gated, and what the ledger answers
//! about an agent's entitlement to it.

use serde_json::{json, Value};

use super::ledger::{check_entitlement, gated, ledger_url, Gate, Unanswered, LEDGER_URL_ENV};

/// Why no report could be given.
#[derive(Debug)]
pub enum StatusFailure {
    /// A declaration of this gateway could not be read; the sentence says which.
    Unreadable(String),
    /// The ledger was asked about an entitlement and gave no answer.
    Unanswered(Unanswered),
}

impl std::fmt::Display for StatusFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StatusFailure::Unreadable(sentence) => formatter.write_str(sentence),
            StatusFailure::Unanswered(error) => write!(formatter, "{error}"),
        }
    }
}

/// The report for `alias` and, when it is named too, `agent_id`'s entitlement
/// to it. `ledger` is the declared origin, or says none is declared and where.
pub async fn status(alias: Option<&str>, agent_id: Option<&str>) -> Result<Value, StatusFailure> {
    let ledger = match ledger_url() {
        Ok(Some(url)) => url,
        Ok(None) => format!("none declared ({LEDGER_URL_ENV})"),
        Err(error) => {
            return Err(StatusFailure::Unreadable(format!("the ledger declaration could not be read: {error}")))
        }
    };
    let mut report = json!({"ledger": ledger});
    let Some(alias) = alias else {
        return Ok(report);
    };
    let gate = gated(alias)
        .map_err(|error| StatusFailure::Unreadable(format!("whether {alias} is gated could not be read: {error}")))?;
    report["alias"] = Value::from(alias);
    report["gated"] = Value::from(gate);
    if let Some(agent) = agent_id {
        report["agent_id"] = Value::from(agent);
        report["entitlement"] = match check_entitlement(agent, alias).await.map_err(StatusFailure::Unanswered)? {
            Gate::Granted { entitlement_id } => json!({"granted": true, "entitlement_id": entitlement_id}),
            Gate::Denied { reason, message } => json!({"granted": false, "reason": reason, "message": message}),
        };
    }
    Ok(report)
}
