//! Each provider declares its reset protocol; when Brama may redeem by
//! itself is the operator's stated policy (`settings.rs`), not a declaration.

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
}
