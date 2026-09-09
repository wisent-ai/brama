//! Walking a ranked candidate list: what the walk has spent, what refused it,
//! and the single refusal an exhausted list is answered with.

use tracing::{info, warn};

use crate::core::failure::POINT_MODEL_SELECTION;
use crate::types::{ModelRequest, ModelResponse};

use super::super::catalogue::route::provider_for;
use super::super::refusal::envelope::refuse;
use super::super::rotation::buffered::attempt_subscription;
use super::super::rotation::streaming::attempt_subscription_stream;
use super::super::routed_stream::RoutedStream;

fn max_selector_models() -> usize {
    "3".parse().expect("valid selector model limit")
}

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
/// so re-dispatching them buys nothing and -- because the model budget is
/// small -- costs the caller every candidate that could still have served. That
/// is exactly how `best` answered `503` naming codex with `attempts: 0` while
/// kimi was serving in the same second.
///
/// So the walk remembers three things: which providers have already emptied,
/// how many provider round trips have actually been paid for, and what each
/// refusal said. A refusal that never reached a provider costs no budget --
/// there is nothing to bound -- and a provider named once is not named twice.
#[derive(Default)]
struct RankedWalk {
    attempts: u32,
    provider_calls: usize,
    refusals: Vec<String>,
    emptied: Vec<String>,
}

impl RankedWalk {
    /// Whether this candidate belongs to a provider that has already refused
    /// everything it holds for this agent.
    fn already_emptied(&self, provider: &str) -> bool {
        self.emptied.iter().any(|seen| seen == provider)
    }

    /// Whether the walk has paid for as many provider round trips as one
    /// selector is allowed to spend.
    fn budget_spent(&self) -> bool {
        self.provider_calls >= max_selector_models()
    }

    /// Record one refused candidate: its cost, its reason, and -- when the
    /// refusal was the provider's whole pool rather than this one route -- the
    /// fact that the rest of that provider's routes need not be asked.
    fn refused(&mut self, model: &str, provider: &str, response: &ModelResponse, emptied: bool) {
        self.attempts = self.attempts.saturating_add(response.attempts);
        if response.attempts > u32::default() {
            self.provider_calls = self.provider_calls.saturating_add(usize::from(true));
        }
        let reason = response.error.as_deref().unwrap_or("failed");
        warn!(
            event = "ranked_candidate_refused",
            model,
            provider,
            pool_emptied = emptied,
            attempts = response.attempts,
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
    /// that was walked past, each with the reason it gave, and the number of
    /// provider attempts that were really made.
    fn into_failure(self, request: &ModelRequest, context: &str) -> ModelResponse {
        let message = if self.refusals.is_empty() {
            context.to_owned()
        } else {
            format!("{context}; {}", self.refusals.join(", "))
        };
        let mut failure = refuse(request, POINT_MODEL_SELECTION, message, None);
        failure.attempts = self.attempts;
        failure
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
        if walk.already_emptied(provider) {
            walk.skipped(&model, provider);
            continue;
        }
        if walk.budget_spent() {
            break;
        }
        let mut candidate = request.clone();
        candidate.model = model.clone();
        let attempt = attempt_subscription(provider, agent_id, &candidate).await;
        match attempt.opened {
            Ok(mut served) => {
                served.attempts = walk.attempts.saturating_add(served.attempts);
                return served;
            }
            Err(response) => walk.refused(&model, provider, &response, attempt.pool_emptied),
        }
    }
    walk.into_failure(request, failure_context)
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
        if walk.already_emptied(provider) {
            walk.skipped(&model, provider);
            continue;
        }
        if walk.budget_spent() {
            break;
        }
        let mut candidate = request.clone();
        candidate.model = model.clone();
        let attempt = attempt_subscription_stream(provider, agent_id, &candidate).await;
        match attempt.opened {
            Ok(mut routed) => {
                routed.attempts = routed.attempts.saturating_add(walk.attempts);
                return Ok(routed);
            }
            Err(response) => walk.refused(&model, provider, &response, attempt.pool_emptied),
        }
    }
    Err(walk.into_failure(request, failure_context))
}
