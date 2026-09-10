//! Brama's HTTP surface: every request a caller can make of this gateway, and
//! the one decision each of them turns on.
//!
//! The families are split by the question they answer, not by the shape of the
//! wire:
//!
//! - [`admission`] — may this request be served at all, and as whom?
//! - [`aliases`] — what does this alias point at, and can that be served here?
//! - [`catalog`] — what may this caller ask for?
//! - [`chat`] — execute one model request, in any of the three formats.
//! - [`streaming`] — turn provider events into one caller-facing SSE body.
//! - [`typed`] — the two endpoints that accept exactly one model name each.
//! - [`refusal`] — how a failure is spelled to a caller.
//! - [`readiness`] — is this process up, and can it carry traffic?
//! - [`telemetry`] — what has this process done?
//! - [`administration`] — the console's write surface.
//! - [`subscriptions`] — the pool: read it, write it, sign one back in.
//! - [`lifecycle`] — assemble the router and open the port.
//!
//! Only the names re-exported below are visible outside this family. Everything
//! else is confined to `crate::core::server` and its descendants, so a handler
//! cannot be reached except through the router that guards it.

mod administration;
mod admission;
mod aliases;
mod catalog;
mod chat;
mod lifecycle;
mod readiness;
mod refusal;
mod streaming;
mod subscriptions;
mod telemetry;
mod typed;

pub use aliases::diagnosis::{
    AliasDiagnosis, ALIAS_CAPABILITY_ABSENT, ALIAS_NO_ROUTE, ALIAS_ROUTES_FILE_INVALID,
    ALIAS_SERVING,
};
pub use aliases::report::{alias_report, AliasReport, AliasReportSource};
pub use aliases::BEST_ALIAS;
pub use lifecycle::start_server;
pub use readiness::check::unroutable_reason;
pub use refusal::contract::{model_error_contract, ModelErrorContract};

pub(crate) use administration::valid_alias;
pub(crate) use aliases::{alias_requires_direct_capability, alias_route_shape_supported};

/// Apply one pool membership document from the local vault-owning CLI.
/// HTTP callers reach the same operation after proving their narrower scope.
pub async fn apply_subscription_membership(
    body: &[u8],
) -> Result<serde_json::Value, serde_json::Value> {
    subscriptions::apply_pool_write(
        crate::subscription_dispatch::pool::PoolScope::Deployment,
        body,
    )
    .await
    .map(|axum::Json(value)| value)
    .map_err(|(_, axum::Json(error))| error)
}
