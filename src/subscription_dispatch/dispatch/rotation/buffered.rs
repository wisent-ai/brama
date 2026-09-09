//! One buffered generation, walked across an agent's bounded credential pool
//! until a provider answers or the pool has nothing left.

use tracing::{info, warn};

use crate::core::failure::{self, IMPACT_MODEL_REQUEST, POINT_CREDENTIAL_SELECTION};
use crate::gateway::broker;
use crate::providers::adapter as provider_registry;
use crate::subscription_dispatch::usage;
use crate::types::{ModelRequest, ModelResponse};
use wisent_errors::Failure;

use super::super::credential::auth_failure::{
    is_auth_failure, is_permanent_auth_failure, mark_credential_revoked,
};
use super::super::ranking::pin::pin_credential;
use super::super::refusal::envelope::remember_failure;
use super::roster::{max_credential_attempts, ordered_candidate_rows};
use super::verdict::{emptied_pool_refusal, PoolObservations};
use super::RouteAttempt;

pub(in crate::subscription_dispatch::dispatch) async fn dispatch_subscription_attempt(
    provider: &str,
    agent_id: &str,
    request: &ModelRequest,
) -> ModelResponse {
    match attempt_subscription(provider, agent_id, request)
        .await
        .opened
    {
        Ok(served) => served,
        Err(failure) => failure,
    }
}

pub(in crate::subscription_dispatch::dispatch) async fn attempt_subscription(
    provider: &str,
    agent_id: &str,
    request: &ModelRequest,
) -> RouteAttempt<ModelResponse> {
    let rows = match ordered_candidate_rows(provider, agent_id, request).await {
        Ok(rows) => rows,
        Err(refusal) => return RouteAttempt::pool_empty(refusal),
    };

    let mut provider_attempts = u32::default();
    // The newest credential-boundary refusal is the operation that finally
    // stopped this request, whether redemption, refresh, or credential decode.
    let mut credential_refusal: Option<Failure> = None;
    // Set when a provider refused a credential outright, as opposed to being out
    // of quota: the two empty the pool for different reasons and the caller has
    // to be told which.
    let mut saw_auth_rejection = false;
    // Set when the vault produced no credential at all -- no capability, no
    // grant, nothing to present. No provider was ever asked, so this is neither
    // capacity nor a provider refusal, and the caller must not be told to wait
    // for it.
    let mut saw_unredeemable_credential = false;
    // Set when a credential was skipped because its recorded block is an
    // authorization block rather than a rate limit. The ledger knows the
    // difference -- `record_reauthorization_needed` writes the state beside the
    // block precisely so it is not lost -- and without reading it here the
    // whole half hour reports as capacity.
    let mut saw_reauthorization_block = false;
    let mut saw_rate_limit_block = false;
    let mut rate_limit_failure = None;
    for (index, entry) in rows.iter().take(max_credential_attempts()).enumerate() {
        let credential_id = &entry.id;
        // A credential inside a recorded block is skipped without a provider
        // call. The previous behaviour re-derived exhaustion from an error
        // string on every request, which meant paying for the 429 to learn what
        // the last 429 already said.
        if usage::is_blocked(credential_id) {
            let reauthorization = usage::needs_reauthorization(credential_id);
            saw_reauthorization_block = saw_reauthorization_block || reauthorization;
            saw_rate_limit_block |= !reauthorization;
            warn!(
                event = "credential_blocked",
                provider,
                credential_index = index,
                reauthorization,
                "bounded credential is inside a recorded block"
            );
            continue;
        }
        let token = match broker::subscription_credential(credential_id, provider).await {
            Ok(token) => token,
            Err(refused) => {
                saw_unredeemable_credential = true;
                warn!(
                    event = "credential_unavailable",
                    provider,
                    credential_index = index,
                    envelope = %refused.to_json(),
                    "{}",
                    refused.render()
                );
                remember_failure(&mut credential_refusal, refused);
                continue;
            }
        };
        let token = match token.expose_utf8() {
            Ok(token) => token,
            Err(error) => {
                saw_unredeemable_credential = true;
                let refused = failure::envelope(
                    POINT_CREDENTIAL_SELECTION,
                    failure::code_for("credential_unauthorized"),
                    IMPACT_MODEL_REQUEST,
                    format!("subscription credential is not valid UTF-8: {error}"),
                )
                .with_context("subscription", credential_id)
                .with_context("provider", provider);
                warn!(
                    event = "credential_invalid_encoding",
                    provider,
                    credential_index = index,
                    envelope = %refused.to_json(),
                    "{}",
                    refused.render()
                );
                remember_failure(&mut credential_refusal, refused);
                continue;
            }
        };
        provider_attempts = provider_attempts.saturating_add(u32::from(true));
        let item = broker::subscription_resource(provider, credential_id);
        let mut result = provider_registry::dispatch(request, &item, token).await;
        result.attempts = provider_attempts;
        usage::record_call(credential_id, provider, &result);

        // Whether the provider refused a token it had just handed us. The
        // string matcher below cannot see that: the message is the same
        // "invalid authentication" either way, and only the sequence says the
        // credential is dead rather than stale.
        let mut rejected_with_fresh_token = false;
        if !result.success && result.error.as_deref().is_some_and(is_auth_failure) {
            match broker::refresh_subscription_credential(credential_id, provider).await {
                Ok(fresh) => match fresh.expose_utf8() {
                    Ok(fresh_token) => {
                        provider_attempts = provider_attempts.saturating_add(u32::from(true));
                        result = provider_registry::dispatch(request, &item, fresh_token).await;
                        result.attempts = provider_attempts;
                        usage::record_call(credential_id, provider, &result);
                        if result.success {
                            info!(
                                event = "credential_refreshed",
                                provider,
                                credential_index = index,
                                "provider request succeeded after forced OAuth refresh"
                            );
                            pin_credential(agent_id, provider, credential_id);
                            return RouteAttempt::served(result);
                        }
                        rejected_with_fresh_token =
                            result.error.as_deref().is_some_and(is_auth_failure);
                    }
                    Err(error) => {
                        let refused = failure::envelope(
                            POINT_CREDENTIAL_SELECTION,
                            failure::code_for("credential_unauthorized"),
                            IMPACT_MODEL_REQUEST,
                            format!("refreshed credential is not valid UTF-8: {error}"),
                        )
                        .with_context("subscription", credential_id)
                        .with_context("provider", provider);
                        remember_failure(&mut credential_refusal, refused);
                    }
                },
                Err(refused) => remember_failure(&mut credential_refusal, refused),
            }
        }
        if result.success {
            if index > 0 {
                info!(
                    event = "credential_rotated",
                    provider,
                    credential_index = index,
                    "provider request succeeded after bounded rotation"
                );
            }
            pin_credential(agent_id, provider, credential_id);
            return RouteAttempt::served(result);
        }
        let error = result.error.clone().unwrap_or_default();
        if is_permanent_auth_failure(&error) {
            warn!(
                event = "credential_retired",
                provider,
                credential_index = index,
                "provider permanently rejected bounded credential"
            );
            mark_credential_revoked(
                credential_id,
                provider,
                "the provider permanently rejected this credential",
            )
            .await;
        }
        if rejected_with_fresh_token {
            // Retried forever otherwise: the refusal reads as an ordinary auth
            // failure, so the next request refreshes and is refused again. One
            // host wrote 275,468 of these lines against a single subscription
            // while every request paid for two provider round trips first.
            warn!(
                event = "credential_retired",
                provider,
                credential_index = index,
                "provider refused a token it had just issued; the subscription needs \
                 re-authorization and is retired until it is granted"
            );
            mark_credential_revoked(
                credential_id,
                provider,
                "the provider refused a token it had just issued",
            )
            .await;
        }
        if is_auth_failure(&error) {
            saw_auth_rejection = true;
            warn!(
                event = "credential_auth_rejected",
                provider,
                credential_index = index,
                "provider rejected bounded credential after OAuth refresh"
            );
            continue;
        }
        let exhausted = error.contains("hit your limit")
            || error.contains("usage limit")
            || error.contains("rate_limit")
            || error.contains("429");
        if !exhausted {
            // The provider answered and refused this request in particular --
            // a malformed body, a model it does not serve. Its other
            // credentials would answer identically, and its other models might
            // not, so this refusal condemns the route and not the pool.
            return RouteAttempt::refused(result);
        }
        usage::record_block(credential_id, provider, &error, &result);
        warn!(
            event = "credential_exhausted",
            provider,
            credential_index = index,
            "provider rejected bounded credential with a rotatable failure"
        );
        rate_limit_failure = Some(result);
    }
    RouteAttempt::pool_empty(emptied_pool_refusal(
        request,
        provider,
        provider_attempts,
        credential_refusal,
        PoolObservations {
            auth_rejection: saw_auth_rejection,
            reauthorization_block: saw_reauthorization_block,
            unredeemable_credential: saw_unredeemable_credential,
            rate_limit_block: saw_rate_limit_block,
        },
        rate_limit_failure,
    ))
}
