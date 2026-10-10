//! Provider-owned reset offers and durable redemption outcomes.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResetCredit {
    pub id: String,
    pub program: String,
    pub remaining_count: u64,
    /// Whether a decrease proves consumption rather than changed eligibility.
    #[serde(default)]
    pub consumption_observable: bool,
    pub usable: bool,
    pub requires_limit: bool,
    pub expires_at_ms: Option<i64>,
    pub clears: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResetOffer {
    pub available_count: u64,
    pub redeemable_count: Option<u64>,
    pub eligible: Option<bool>,
    /// Explicit provider admission state; a full-looking window is not a refusal.
    pub limit_reached: Option<bool>,
    pub reason: Option<String>,
    pub next_credit_id: Option<String>,
    pub credits: Vec<ResetCredit>,
    pub observed_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResetObservation {
    pub offer: Option<ResetOffer>,
    pub attempted_at_ms: i64,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResetState {
    Requested,
    Unconfirmed,
    ConsumptionObserved,
    Redeemed,
    Refused,
}

impl ResetState {
    pub fn unresolved(self) -> bool {
        matches!(self, Self::Requested | Self::Unconfirmed)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResetRedemption {
    pub provider: String,
    pub member: String,
    pub credit_id: String,
    pub remaining_before: u64,
    pub request_id: String,
    pub reason: String,
    pub state: ResetState,
    pub detail: Option<String>,
    pub at_ms: i64,
}
