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

    pub fn failure_code(&self) -> Code {
        match self {
            Self::WelesUnreachable { .. } => Code::InfraDown,
            Self::Operation {
                status: Some(401 | 403),
                ..
            } => Code::Auth,
            Self::Operation {
                status: Some(429), ..
            } => Code::RateLimit,
            Self::Operation {
                status: Some(500..=599),
                ..
            } => Code::InfraDown,
            _ => Code::Config,
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
        serde_json::from_str(&envelope.to_json()).expect("Wisent failure serialization is JSON")
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
