//! Each provider declares its reset protocol and the operator's policy keys.

use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetProtocol {
    CodexWham,
    ClaudePrograms,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ResetDeclaration {
    pub protocol: ResetProtocol,
    pub listing_path: String,
    pub usage_path: Option<String>,
    pub alternate_listing_path: Option<String>,
    pub profile_path: Option<String>,
    pub redemption_path: String,
    pub auto_redeem: bool,
    pub keep_credits: Option<String>,
    pub min_blocked_minutes: Option<String>,
    pub salvage_horizon_hours: Option<String>,
}
