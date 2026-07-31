//! Gateway HTTP surface. Only the broker client remains; the trade/subscription
//! product routes were excised when brama became a pure model gateway.
pub mod broker;
mod oauth_refresh;
