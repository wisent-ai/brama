//! Which routes this deployment can actually serve, and who owns each one.
//!
//! `route` answers the cheap question -- which provider a canonical
//! `provider/model` string names, and whether that provider may only be billed
//! to a signed caller. `cache` holds what discovery has already learned and how
//! long that answer stands. `subscription_models` asks the providers an agent's
//! own subscriptions point at; `console` answers the bearer-authenticated
//! console, which has no agent identity to ask on behalf of, from those same
//! caches plus this gateway's directly-keyed providers.

pub(super) mod cache;
pub(super) mod console;
pub(super) mod route;
pub(super) mod subscription_models;
