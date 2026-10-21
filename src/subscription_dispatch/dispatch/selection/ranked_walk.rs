//! Walking a ranked candidate list: what the walk has spent, what refused it,
//! and the single refusal an exhausted list is answered with.

use tracing::{info, warn};

use crate::core::failure::POINT_MODEL_SELECTION;
use crate::types::{GatewayRefusal, ModelRequest, ModelResponse, ProviderRefusal, Refusal};

use super::super::catalogue::cache::discovery_failure;
use super::super::catalogue::route::provider_for;
use super::super::refusal::envelope::refuse_as;
use super::super::rotation::buffered::attempt_subscription;
use super::super::rotation::streaming::attempt_subscription_stream;
use super::super::routed_stream::RoutedStream;

/// The sentence a selector that walked its whole candidate list opens with.
/// One string per selector, shared by the buffered and streaming walks, so the
/// two cannot drift into describing the same exhausted list differently.
pub(super) const ANY_SUBSCRIPTION_CONTEXT: &str = "no working subscription model for signed agent";
pub(super) const ANY_VISION_CONTEXT: &str =
    "no working vision-capable subscription model for signed agent";

/// What one ranked walk has spent and what refused it so far.
///
/// A ranked selector is a list of routes, and several of those routes belong to
/// the same provider. When a provider's whole credential pool empties, every
/// remaining route of that provider will be refused for the identical reason,
/// so re-dispatching them buys nothing; the walk skips them and asks every
/// other candidate on the list.
///
/// So the walk remembers which providers have already emptied, how many
/// attempts were made, and what each refusal said; a provider named once is
/// not named twice.
#[derive(Default)]
struct RankedWalk {
    attempts: u32,
    refusals: Vec<String>,
    classes: Vec<ProviderRefusal>,
    emptied: Vec<String>,
    /// Every provider at least one candidate route belonged to.
    candidates: Vec<String>,
}

impl RankedWalk {
    /// Whether this candidate belongs to a provider that has already refused
    /// everything it holds for this agent.
    fn already_emptied(&self, provider: &str) -> bool {
        self.emptied.iter().any(|seen| seen == provider)
    }

    /// Note that a route of `provider` was among the candidates.
    fn candidate(&mut self, provider: &str) {
        if !self.candidates.iter().any(|seen| seen == provider) {
            self.candidates.push(provider.to_owned());
        }
    }

    /// Record one refused candidate: its cost, its reason, how long asking it
    /// took, and -- when the refusal was the provider's whole pool rather
    /// than this one route -- the fact that the rest of that provider's
    /// routes need not be asked.
    fn refused(
        &mut self,
        model: &str,
        provider: &str,
        refusal: &Refusal,
        attempts: u32,
        emptied: bool,
        elapsed_ms: u128,
    ) {
        self.attempts = self.attempts.saturating_add(attempts);
        let reason = refusal.message.as_str();
        self.classes.push(refusal.class);
        warn!(
            event = "ranked_candidate_refused",
            model,
            provider,
            pool_emptied = emptied,
            attempts,
            elapsed_ms,
            reason,
            "ranked selector walked past a refused candidate"
        );
        if emptied {
            if self.already_emptied(provider) {
                return;
            }
            self.emptied.push(provider.to_owned());
            // The provider's name, not the route's: the pool refused every
            // model behind it, and naming one of them would read as if the
            // others were untried.
            self.refusals.push(format!("{provider} refused ({reason})"));
            return;
        }
        self.refusals.push(format!("{model} refused ({reason})"));
    }

    /// Note a candidate skipped without a provider call, so the log shows the
    /// walk passing it rather than never reaching it.
    fn skipped(&self, model: &str, provider: &str) {
        info!(
            event = "ranked_candidate_skipped",
            model,
            provider,
            "provider pool already emptied for this request; candidate skipped unasked"
        );
    }

    /// The refusal an exhausted candidate list answers with: every provider
    /// that was walked past, each with the reason it gave, every member the
    /// walk never reached because no route of its provider was a candidate,
    /// each with the reason the pool records, and the number of provider
    /// attempts that were really made.
    fn into_failure(self, agent_id: &str, request: &ModelRequest, context: &str) -> ModelResponse {
        let unreached = unreached(agent_id, &self.candidates);
        let named = self
            .refusals
            .into_iter()
            .chain(unreached)
            .collect::<Vec<_>>();
        let message = if named.is_empty() {
            context.to_owned()
        } else {
            format!("{context}; {}", named.join(", "))
        };
        let class = walked_class(&self.classes);
        let mut failure = refuse_as(request, POINT_MODEL_SELECTION, class, message, None);
        failure.attempts = self.attempts;
        failure
    }
}

/// The members of the routing pool whose provider had no candidate route, each
/// with why it was not one: its recorded status, its retirement, the ledger's
/// standing refusal, the last model discovery's refusal, or a discovery that
/// published no model for it, so a pool holding accounts of a provider the
/// walk never reached never reads as if those accounts did not exist.
fn unreached(agent_id: &str, walked: &[String]) -> Vec<String> {
    let entries = match crate::gateway::broker::routing_subscriptions(agent_id) {
        Ok(entries) => entries,
        Err(error) => {
            let message = format!(
                "the subscription inventory could not be read to name the members not tried: {error}"
            );
            return vec![message];
        }
    };
    entries
        .into_iter()
        .filter(|entry| {
            !walked
                .iter()
                .any(|provider| provider == entry.provider.trim())
        })
        .map(|entry| {
            let provider = entry.provider.trim();
            let retired = crate::journal::is_retired(&entry.id);
            let cause = if entry.status != "active" {
                format!("its status is {}", entry.status)
            } else if let Err(unread) = &retired {
                format!("whether its owner retired it could not be read: {unread}")
            } else if retired == Ok(true) {
                "it is retired".to_owned()
            } else if let Some(refused) =
                crate::subscription_dispatch::usage::standing_refusal(&entry.id)
            {
                format!("the ledger refuses it: {}", refused.message)
            } else if let Some(failure) = discovery_failure(provider, &entry.id) {
                format!("its model discovery was refused: {failure}")
            } else {
                "its model discovery published no model".to_owned()
            };
            format!("{} ({provider}) not tried: {cause}", entry.id)
        })
        .collect()
}

/// The class an exhausted walk is answered with, from the classes its
/// candidates were refused with.
///
/// A spent paid balance comes first because no wait repairs it; a rate window
/// next, because a wait does and the caller should take it; then the
/// authorization failures, which only an operator repairs and which must never
/// be dressed as capacity. A walk that saw none of these found nothing that
/// could serve it right now.
fn walked_class(classes: &[ProviderRefusal]) -> ProviderRefusal {
    let saw = |class: ProviderRefusal| classes.contains(&class);
    let unavailable = ProviderRefusal::Gateway(GatewayRefusal::SubscriptionUnavailable);
    let unauthorized = ProviderRefusal::Gateway(GatewayRefusal::CredentialUnauthorized);
    let reauthorize =
        ProviderRefusal::Gateway(GatewayRefusal::SubscriptionReauthorizationRequired);
    if saw(ProviderRefusal::QuotaExhausted) {
        ProviderRefusal::QuotaExhausted
    } else if saw(ProviderRefusal::RateLimited) {
        ProviderRefusal::RateLimited
    } else if saw(unavailable) || !(saw(unauthorized) || saw(reauthorize)) {
        unavailable
    } else if saw(unauthorized) {
        unauthorized
    } else {
        reauthorize
    }
}

/// Walk a ranked candidate list until one of them serves.
///
/// A candidate that cannot be redeemed is walked past, not fatal: only an
/// exhausted list is a failed request, and the failure names every provider
/// the walk went past and what each of them said.
pub(super) async fn dispatch_ranked_models(
    agent_id: &str,
    request: &ModelRequest,
    models: Vec<String>,
    failure_context: &str,
) -> ModelResponse {
    let mut walk = RankedWalk::default();
    for model in models {
        let Some(provider) = provider_for(&model) else {
            walk.refusals
                .push(format!("{model} refused (unknown provider/model route)"));
            continue;
        };
        walk.candidate(provider);
        if walk.already_emptied(provider) {
            walk.skipped(&model, provider);
            continue;
        }
        let mut candidate = request.clone();
        candidate.model = model.clone();
        let asking = std::time::Instant::now();
        let attempt = attempt_subscription(provider, agent_id, &candidate).await;
        match attempt.opened {
            Ok(mut served) => {
                served.attempts = walk.attempts.saturating_add(served.attempts);
                return served;
            }
            // A route is walked past on its refusal; an attempt that holds
            // none is an answer, and is returned as one.
            Err(response) => match &response.refusal {
                Some(refusal) => walk.refused(
                    &model,
                    provider,
                    refusal,
                    response.attempts,
                    attempt.pool_emptied,
                    asking.elapsed().as_millis(),
                ),
                None => return response,
            },
        }
    }
    walk.into_failure(agent_id, request, failure_context)
}

/// The streaming counterpart of [`dispatch_ranked_models`]: the first model
/// whose stream commits wins, and a model that fails before its first byte
/// costs the caller nothing but an attempt count.
pub(super) async fn dispatch_ranked_models_stream(
    agent_id: &str,
    request: &ModelRequest,
    models: Vec<String>,
    failure_context: &str,
) -> Result<RoutedStream, ModelResponse> {
    let mut walk = RankedWalk::default();
    for model in models {
        let Some(provider) = provider_for(&model) else {
            walk.refusals
                .push(format!("{model} refused (unknown provider/model route)"));
            continue;
        };
        walk.candidate(provider);
        if walk.already_emptied(provider) {
            walk.skipped(&model, provider);
            continue;
        }
        let mut candidate = request.clone();
        candidate.model = model.clone();
        let asking = std::time::Instant::now();
        let attempt = attempt_subscription_stream(provider, agent_id, &candidate).await;
        match attempt.opened {
            Ok(mut routed) => {
                routed.attempts = routed.attempts.saturating_add(walk.attempts);
                return Ok(routed);
            }
            Err(response) => match &response.refusal {
                Some(refusal) => walk.refused(
                    &model,
                    provider,
                    refusal,
                    response.attempts,
                    attempt.pool_emptied,
                    asking.elapsed().as_millis(),
                ),
                None => return Err(response),
            },
        }
    }
    Err(walk.into_failure(agent_id, request, failure_context))
}
