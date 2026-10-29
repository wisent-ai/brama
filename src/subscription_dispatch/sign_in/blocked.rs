//! Authentication failures describe an observed operation, not a missing local account table.
use crate::core::failure;
use serde_json::{json, Value};
use std::fmt;
use wisent_errors::Code;

pub const POINT: &str = "brama.subscriptions.automatic-sign-in";
pub const IMPACT: &str = "one automatic sign-in";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    Operation {
        code: String,
        stage: String,
        detail: String,
        status: Option<u16>,
    },
    WelesUnreachable {
        detail: String,
    },
    NotPlacedHost {
        placed_on: String,
        this_host: String,
    },
}

impl Blocked {
    pub fn code(&self) -> &str {
        match self {
            Self::Operation { code, .. } => code,
            Self::WelesUnreachable { .. } => "weles_unreachable",
            Self::NotPlacedHost { .. } => "not_placed_host",
        }
    }

    /// The envelope code a refused sign-in carries, classified by the HTTP
    /// status it observed as `reqwest::StatusCode` names them.
    pub fn failure_code(&self) -> Code {
        let raw = match self {
            Self::WelesUnreachable { .. } => return Code::InfraDown,
            Self::Operation {
                status: Some(raw), ..
            } => *raw,
            _ => return Code::Config,
        };
        match reqwest::StatusCode::from_u16(raw) {
            Ok(reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN) => Code::Auth,
            Ok(reqwest::StatusCode::TOO_MANY_REQUESTS) => Code::RateLimit,
            Ok(status) if status.is_server_error() => Code::InfraDown,
            Ok(_other_status) | Err(_not_a_status) => Code::Config,
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::Operation { detail, .. } | Self::WelesUnreachable { detail } => detail.clone(),
            Self::NotPlacedHost { placed_on, this_host } => format!(
                "This Brama process runs on {this_host}; Stado places the serving gateway on {placed_on}"
            ),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut result = json!({"blocked_by": self.code(), "detail": self.detail()});
        if let Self::Operation { stage, status, .. } = self {
            result["stage"] = json!(stage);
            result["http_status"] = json!(status);
        }
        result
    }

    pub fn failure(&self, subscription_id: Option<&str>) -> Value {
        let mut envelope = failure::envelope(POINT, self.failure_code(), IMPACT, self.detail())
            .with_context("blocked_by", self.code());
        if let Some(id) = subscription_id {
            envelope = envelope.with_context("subscription", id);
        }
        if let Self::Operation { stage, status, .. } = self {
            envelope = envelope.with_context("stage", stage);
            if let Some(status) = status {
                envelope = envelope.with_context("http_status", status.to_string());
            }
        }
        failure::value(&envelope)
    }
}

impl fmt::Display for Blocked {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail())
    }
}

#[derive(Debug, Clone)]
pub enum SignInError {
    Blocked(Blocked),
    Dependency(String),
}
impl SignInError {
    pub fn blocked(&self) -> Option<&Blocked> {
        match self {
            Self::Blocked(value) => Some(value),
            Self::Dependency(_) => None,
        }
    }
}
impl fmt::Display for SignInError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked(value) => write!(formatter, "{value}"),
            Self::Dependency(value) => formatter.write_str(value),
        }
    }
}
impl From<Blocked> for SignInError {
    fn from(value: Blocked) -> Self {
        Self::Blocked(value)
    }
}
impl From<String> for SignInError {
    fn from(value: String) -> Self {
        Self::Dependency(value)
    }
}

/// When a held member's failure was observed, so a refusal repeated after
/// the vault or the executor changed is read as the last observation and
/// not as the state of now.
pub(super) fn observed_at(at_ms: Option<i64>, verb: &str) -> String {
    match at_ms.and_then(chrono::DateTime::from_timestamp_millis) {
        Some(at) => format!("{verb} at {}", at.to_rfc3339()),
        None => format!("{verb} at an unrecorded time"),
    }
}

/// The hold an unreadable journal puts on a member: its sign-in history is
/// unknown, so it is neither cleared nor blamed on the provider.
pub(super) fn journal_unreadable(cause: String) -> Blocked {
    Blocked::Operation {
        code: "journal_unreadable".into(),
        stage: "journal".into(),
        detail: cause,
        status: None,
    }
}

/// Each member's sign-in state with the dispatch hold it is under now: a
/// spent window or a rate limit names the instant it lifts and the
/// provider's own words, so a refusal does not send its reader to repair a
/// sign-in for a member that only waits for its window to reset.
pub(super) fn with_holds(ids: &[&String], states: Vec<String>) -> Vec<String> {
    ids.iter()
        .zip(states)
        .map(|(id, state)| match held(id) {
            Some(hold) => format!("{state}, {hold}"),
            None => state,
        })
        .collect()
}

fn held(id: &str) -> Option<String> {
    let block = crate::subscription_dispatch::usage::usage_for(id)?.block?;
    if block.blocked_until_ms <= chrono::Utc::now().timestamp_millis() {
        return None;
    }
    let until = chrono::DateTime::from_timestamp_millis(block.blocked_until_ms)?;
    Some(format!(
        "held until {} by the provider's answer: {}",
        until.to_rfc3339(),
        block.reason
    ))
}
