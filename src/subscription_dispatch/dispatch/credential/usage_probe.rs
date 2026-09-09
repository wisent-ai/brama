//! One provider call spent on exactly one named account, so an operator can
//! ask what that account's plan says without rotating onto another.

use crate::core::failure::POINT_CREDENTIAL_SELECTION;
use crate::gateway::broker;
use crate::providers::adapter as provider_registry;
use crate::subscription_dispatch::usage;
use crate::types::{ModelRequest, ModelResponse};

use super::super::refusal::envelope::{failure_detail, refuse};

/// Spend one provider call on exactly one subscription, for the on-demand usage
/// probe.
///
/// Three differences from the request path, each of them deliberate. It never
/// rotates: a probe asks a named account what its plan says, and answering with
/// the window of whichever sibling credential happened to work would attribute
/// one account's quota to another. It returns the provider's own sentence rather
/// than the rotation summary, because the sentence -- "OAuth access token has
/// been revoked" against "token is expired" -- is the entire reason the probe
/// exists. And it neither forces a refresh nor retires anything: a check an
/// operator asked for must not change which credentials the router will pick. An
/// access token that merely expired is still renewed, because redemption does
/// that for every caller before the call is made.
///
/// What it shares is the part that matters: the same provider dispatch and the
/// same ledger path real traffic writes to, so a probe's plan windows are
/// recorded exactly as a request's are -- attributed to the probe, because a
/// window somebody asked for and a window a caller's work revealed are not the
/// same statement about the account.
pub async fn probe_subscription_usage(
    subscription_id: &str,
    provider: &str,
    request: &ModelRequest,
) -> ModelResponse {
    let token = match broker::subscription_credential(subscription_id, provider).await {
        Ok(token) => token,
        Err(refused) => {
            return refuse(
                request,
                POINT_CREDENTIAL_SELECTION,
                failure_detail(&refused),
                Some(refused),
            );
        }
    };
    let token = match token.expose_utf8() {
        Ok(token) => token,
        Err(error) => {
            return refuse(
                request,
                POINT_CREDENTIAL_SELECTION,
                format!("credential is not valid UTF-8: {error}"),
                None,
            );
        }
    };
    let item = broker::subscription_resource(provider, subscription_id);
    let mut result = provider_registry::dispatch(request, &item, token).await;
    result.attempts = u32::from(true);
    usage::record_call_from(
        subscription_id,
        provider,
        &result,
        usage::UsageSource::Probe,
    );
    result
}
