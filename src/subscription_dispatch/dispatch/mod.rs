//! Route canonical `provider/model` requests through stateless provider APIs.
//!
//! One request crosses six seams here, and each is a family of its own.
//! `catalogue` says which routes exist and who owns them; `ranking` puts the
//! candidates in the order the ledger justifies; `selection` walks that order
//! for the caller; `rotation` spends one route across the bounded credential
//! pool an agent holds for its provider; `credential` answers what can be
//! presented at all; and `refusal` writes the one sentence a broken call is
//! reported with. `direct_route` is the caller-independent path this gateway
//! pays for itself, and `routed_stream` is the committed stream every
//! streaming path hands back.
//!
//! Nothing here walks a second route after the first one fails. A refused
//! route is refused: the caller is told which provider broke and why, rather
//! than being served by a model it did not ask for. The one list this module
//! does walk is a selector's own candidate list, which the caller asked for by
//! naming `any`, `best`, `any-vision-capable` or `task:<name>` instead of a
//! route.

mod caller_identity;
mod catalogue;
mod credential;
mod direct_route;
mod ranking;
mod refusal;
mod rotation;
mod routed_stream;
mod selection;

pub(crate) use caller_identity::authenticate_agent;
pub use catalogue::cache::discovery_failure;
pub use catalogue::console::registry_models_for_console;
pub use catalogue::route::{is_subscription_model, provider_requires_caller_identity};
pub(crate) use catalogue::route::provider_for;
pub use catalogue::subscription_models::registry_models_for_agent;
pub use credential::readiness::probe_subscription_redemption;
pub use credential::usage_probe::probe_subscription_usage;
pub use direct_route::{dispatch_direct, dispatch_direct_openai_typed, dispatch_direct_stream};
pub use ranking::candidates::{
    active_supported_models_for_agent, active_vision_capable_models_for_agent,
};
pub use refusal::pool_empty::{pool_empty_summary, PoolEmptyCause};
pub(crate) use refusal::pool_empty::no_active_credential_summary;
pub use routed_stream::RoutedStream;
pub use selection::buffered::{
    dispatch_any_subscription, dispatch_any_vision_capable_subscription, dispatch_best_subscription,
    dispatch_best_subscription_for_agent, dispatch_subscription, dispatch_subscription_for_agent,
    dispatch_task_subscription,
};
pub use selection::streaming::{
    dispatch_any_subscription_stream, dispatch_any_vision_capable_subscription_stream,
    dispatch_best_subscription_stream, dispatch_best_subscription_stream_for_agent,
    dispatch_subscription_stream, dispatch_subscription_stream_for_agent,
    dispatch_task_subscription_stream,
};
