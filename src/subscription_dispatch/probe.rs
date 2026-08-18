//! Spend one deliberately tiny completion against one subscription, when an
//! operator asks for it and not otherwise.
//!
//! A provider's free usage report says how much of a plan is gone; it does not
//! say whether the provider will actually serve this credential right now. Those
//! are different questions, and the second one can only be answered by making a
//! real request. That request costs quota, which is why nothing here runs on a
//! timer any more: [`crate::subscription_dispatch::plan_usage`] keeps every row
//! current for free, and this module exists for the moment an operator wants the
//! stronger answer about one named account.
//!
//! The trigger is an admin route rather than a `brama` subcommand, and that is
//! the deliberate choice. Redeeming a subscription credential needs the
//! capability ids and request-signing identities the launcher installs into the
//! serving process's own environment, and a standalone desktop install passes its
//! provider credentials over that process's standard input, where they stay in
//! its memory and nowhere else. A subcommand started by hand has none of that: it
//! would report a credential failure that says nothing about the account, which
//! is precisely the kind of unreadable blank this whole area exists to remove.
//! The console that renders `probe` is also the surface an operator is looking at
//! when the question occurs to them.
//!
//! Two rules survive from the timed version, because both are about not doing
//! harm to learn a statistic. A subscription inside a recorded block is refused
//! rather than probed: the block exists because the provider said the account is
//! out of quota, and spending a request to re-read that sentence is what the
//! block was introduced to stop. And the request is the smallest the provider
//! will accept, logged under its own event, so nobody's usage report has to
//! explain it.

use serde_json::Value;
use tracing::{info, warn};

use crate::providers::adapter as provider_registry;
use crate::subscription_dispatch::dispatch::probe_subscription_usage;
use crate::subscription_dispatch::usage::{self, Probe};
use crate::types::{Message, ModelRequest};

/// The smallest output budget that is accepted everywhere. Anthropic allows one
/// token; the OpenAI Responses API that Codex speaks rejects anything below
/// sixteen, and a probe refused for its own shape would report a broken
/// credential where there is none.
const PROBE_MAX_TOKENS: u32 = 16;
/// The probe reads response headers and discards the body, so the prompt only
/// has to be a well-formed turn.
const PROBE_PROMPT: &str = "ping";

/// Ask one subscription's provider for a real answer, once, and record the
/// verdict.
///
/// The verdict is returned as well as recorded: an operator who triggered this
/// is waiting for it, and reading it back out of the ledger would be a second
/// answer to the same question.
pub async fn probe_once(subscription_id: &str, provider: &str) -> Result<Probe, String> {
    if usage::is_blocked(subscription_id) {
        info!(
            event = "usage_probe_skipped_blocked",
            subscription = %subscription_id,
            provider = %provider,
            "a recorded block is still in force; not spending a probe against a \
             rate-limited account"
        );
        return Err(
            "this subscription is inside a recorded rate-limit block, which already says the \
             account is out of quota"
                .to_string(),
        );
    }
    let Some(model) = provider_registry::plan_probe_route(provider) else {
        return Err(format!(
            "provider `{provider}` names no model a probe can be spent on"
        ));
    };
    let request = ModelRequest {
        messages: vec![Message {
            role: "user".into(),
            content: Value::String(PROBE_PROMPT.to_string()),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }],
        model,
        max_tokens: PROBE_MAX_TOKENS,
        temperature: 0.0,
        system: None,
        tools: None,
        tool_choice: None,
        // The subscription is pinned by the probe's own dispatch, not by a
        // billing target: nothing about this request is billable work a caller
        // asked for.
        billing_target: None,
    };
    let response = probe_subscription_usage(subscription_id, provider, &request).await;
    let detail = response.error.as_deref();
    let probe = usage::record_probe(subscription_id, provider, response.success, detail);
    if response.success {
        info!(
            event = "usage_probe_recorded",
            subscription = %subscription_id,
            provider = %provider,
            windows = response.limits.len(),
            "the provider answered an operator's plan usage probe"
        );
        return Ok(probe);
    }
    warn!(
        event = "usage_probe_refused",
        subscription = %subscription_id,
        provider = %provider,
        detail = detail.unwrap_or("the provider gave no reason"),
        "an operator's plan usage probe was refused; the row can now say why instead of \
         reporting an empty plan"
    );
    Ok(probe)
}
