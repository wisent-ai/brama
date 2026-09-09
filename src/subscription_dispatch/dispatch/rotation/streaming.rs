//! One streaming generation, opened across an agent's bounded credential pool.
//! Every rotation happens before the first caller byte, because after it none
//! is possible.

use std::time::Instant;
use tracing::{info, warn};

use crate::core::failure::{self, IMPACT_MODEL_REQUEST, POINT_CREDENTIAL_SELECTION};
use crate::gateway::broker;
use crate::providers::adapter as provider_registry;
use crate::subscription_dispatch::usage;
use crate::types::ModelRequest;
use wisent_errors::Failure;

use super::super::credential::auth_failure::{
    is_auth_failure, is_permanent_auth_failure, mark_credential_revoked,
};
use super::super::ranking::pin::pin_credential;
use super::super::refusal::envelope::remember_failure;
use super::super::routed_stream::{spawn_stream_recorder, RoutedStream};
use super::roster::{max_credential_attempts, ordered_candidate_rows};
use super::verdict::{emptied_pool_refusal, PoolObservations};
use super::RouteAttempt;

pub(in crate::subscription_dispatch::dispatch) async fn attempt_subscription_stream(
    provider: &str,
    agent_id: &str,
    request: &ModelRequest,
) -> RouteAttempt<RoutedStream> {
    let rows = match ordered_candidate_rows(provider, agent_id, request).await {
        Ok(rows) => rows,
        Err(refusal) => return RouteAttempt::pool_empty(refusal),
    };

    let mut provider_attempts = u32::default();
    let mut credential_refusal: Option<Failure> = None;
    let mut saw_auth_rejection = false;
    let mut saw_unredeemable_credential = false;
    // The streaming path empties its pool the same way the buffered one does,
    // so it reads the ledger's authorization state the same way. Fixing one and
    // not the other would make the same broken credential answer `503` to a
    // buffered caller and `429` to a streaming one.
    let mut saw_reauthorization_block = false;
    let mut saw_rate_limit_block = false;
    let mut rate_limit_failure = None;
    for (index, entry) in rows.iter().take(max_credential_attempts()).enumerate() {
        let credential_id = &entry.id;
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
        let started = Instant::now();
        let mut result = provider_registry::dispatch_stream(request, &item, token).await;
        // An auth refusal before the first byte gets the same one forced
        // refresh the buffered path allows, and the retry is just as safe:
        // the caller has seen nothing either way.
        let mut rejected_with_fresh_token = false;
        if result
            .as_ref()
            .err()
            .and_then(|failure| failure.error.as_deref())
            .is_some_and(is_auth_failure)
        {
            match broker::refresh_subscription_credential(credential_id, provider).await {
                Ok(fresh) => match fresh.expose_utf8() {
                    Ok(fresh_token) => {
                        provider_attempts = provider_attempts.saturating_add(u32::from(true));
                        result =
                            provider_registry::dispatch_stream(request, &item, fresh_token).await;
                        rejected_with_fresh_token = result
                            .as_ref()
                            .err()
                            .and_then(|failure| failure.error.as_deref())
                            .is_some_and(is_auth_failure);
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
        let failure = match result {
            Ok(stream) => {
                if index > 0 {
                    info!(
                        event = "credential_rotated",
                        provider,
                        credential_index = index,
                        "provider stream committed after bounded rotation"
                    );
                }
                pin_credential(agent_id, provider, credential_id);
                return RouteAttempt::served(RoutedStream {
                    model: request.model.clone(),
                    attempts: provider_attempts,
                    events: spawn_stream_recorder(
                        credential_id,
                        provider,
                        &request.model,
                        provider_attempts,
                        started,
                        stream,
                    ),
                });
            }
            Err(failure) => failure,
        };
        usage::record_call(credential_id, provider, &failure);
        let error = failure.error.clone().unwrap_or_default();
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
            // Same loop guard as the buffered path: a refusal of a token the
            // provider had just issued reads as an ordinary auth failure and
            // would otherwise be refreshed and refused on every request.
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
            // As in the buffered path: the provider answered and refused this
            // one route, which says nothing about its other models.
            return RouteAttempt::refused(failure);
        }
        usage::record_block(credential_id, provider, &error, &failure);
        warn!(
            event = "credential_exhausted",
            provider,
            credential_index = index,
            "provider rejected bounded credential with a rotatable failure"
        );
        rate_limit_failure = Some(failure);
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
