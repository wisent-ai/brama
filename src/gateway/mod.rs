//! Gateway HTTP surface. Only the broker client remains; the trade/subscription
//! product routes were excised when brama became a pure model gateway.
pub mod broker;
mod oauth_refresh;

pub use oauth_refresh::renewable;
pub(crate) use oauth_refresh::jwt_claims;
