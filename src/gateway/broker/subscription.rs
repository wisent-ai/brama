//! Subscriptions: which ones exist, and what happens to the one behind a call.
//!
//! A subscription is a paid account this deployment may spend, and the four
//! things it needs are here as four modules because they change for different
//! reasons. `account` is the row every audience reads and the parsers each
//! source needs to produce it. `inventory` answers which subscriptions an
//! agent has, and caches that answer. `renewal` replaces a grant the provider
//! is about to stop accepting, and records the verdict. `tags` states what a
//! credential write must leave behind for discovery to find the account at
//! all, and `donation` is the sign-in's own path: the row it records and the
//! credential it banks.
//!
//! Redeeming a credential is not here -- that is the file above, because the
//! same redemption serves providers and subscriptions alike.

mod account;
mod donation;
mod inventory;
mod renewal;
mod tags;

// The subscription surface `broker` has always published, named one at a time
// so every existing caller path stays exactly what it was.
pub use account::{SubscriptionEntry, UnroutableAccount};
pub use donation::{
    donated_add, donated_remove, donated_subscriptions_path, put_donated_credential,
    DonationRefusal,
};
pub use inventory::{
    discover_subscriptions, list_all_subscriptions, list_recoverable_subscriptions,
    list_subscriptions, list_unroutable_accounts,
};
pub use renewal::{
    refresh_subscription_credential, refresh_subscription_credential_ahead, supports_oauth_refresh,
    RefreshAhead,
};
pub use tags::subscription_tags_for_write;

// What the rest of `broker` reaches in here: the deployment's declared
// subscription ids, and the rotation the redemption path forces.
pub(super) use account::configured_subscription_ids;
pub(super) use renewal::refresh_subscription_credential_inner;
