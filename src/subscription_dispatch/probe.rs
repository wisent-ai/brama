//! Ask every subscription what its plan says, instead of waiting for a client to
//! ask for it.
//!
//! A provider states its plan windows in the headers of an ordinary answer, so
//! before this existed a window was recorded only for the account that happened
//! to serve a request. On a fleet of seven subscriptions that meant one row with
//! a window and six blank ones, and the blanks were unreadable: a provider that
//! publishes nothing, an account no traffic ever reached, and a credential the
//! provider refuses all render as the same absence. This module spends one
//! deliberately tiny request per subscription on a timer so that each row can
//! state which of those it is.
//!
//! Three rules hold here, and each of them is about not doing harm to learn a
//! statistic. A subscription inside a recorded block is not probed at all: the
//! block exists precisely because the provider said it is out of quota, and
//! spending a request to re-read that sentence is what the block was introduced
//! to stop. The request is the smallest the provider will accept and is logged
//! under its own event, so nobody's usage report has to explain it. And one
//! subscription's failure -- including a panic -- ends that subscription's probe
//! and nothing else.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::Value;
use tracing::{info, warn};

use crate::gateway::broker;
use crate::providers::adapter as provider_registry;
use crate::subscription_dispatch::dispatch::probe_subscription_usage;
use crate::subscription_dispatch::usage;
use crate::types::{Message, ModelRequest};

const INTERVAL_ENV: &str = "BRAMA_USAGE_PROBE_INTERVAL_SECS";
/// A quarter hour: shorter than every plan window any provider publishes, so a
/// window never ages past its own reset before it is read again, and long enough
/// that the cost of knowing is a handful of requests an hour per account.
const DEFAULT_INTERVAL_SECS: u64 = 15 * 60;
/// Long enough for the listener to be bound and the entitlements router to have
/// answered its first read, so the first sweep measures the gateway's steady
/// state rather than its startup.
const STARTUP_DELAY_SECS: u64 = 30;
/// The smallest output budget that is accepted everywhere. Anthropic allows one
/// token; the OpenAI Responses API that Codex speaks rejects anything below
/// sixteen, and a probe refused for its own shape would report a broken
/// credential where there is none.
const PROBE_MAX_TOKENS: u32 = 16;
/// The probe reads response headers and discards the body, so the prompt only
/// has to be a well-formed turn.
const PROBE_PROMPT: &str = "ping";

/// How often to sweep every subscription, or `None` when probing is off.
fn interval() -> Option<Duration> {
    let seconds = std::env::var(INTERVAL_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECS);
    // Zero is the documented off switch rather than a busy loop: a host that
    // must not spend requests on statistics says so with a number.
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

/// Start the usage probe, unless this host has turned it off.
pub fn spawn() {
    let Some(period) = interval() else {
        info!(
            event = "usage_probe_disabled",
            env = INTERVAL_ENV,
            "proactive plan usage probing is off; plan windows will only be recorded \
             for subscriptions that serve real traffic"
        );
        return;
    };
    info!(
        event = "usage_probe_scheduled",
        interval_secs = period.as_secs(),
        "proactive plan usage probing is on"
    );
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_DELAY_SECS)).await;
        loop {
            sweep().await;
            tokio::time::sleep(period).await;
        }
    });
}

/// Probe every active subscription the gateway knows about, once.
async fn sweep() {
    let mut probed = BTreeSet::new();
    for agent in broker::configured_request_sign_agents() {
        for entry in broker::list_subscriptions(&agent).await {
            if entry.status != "active" || crate::journal::is_retired(&entry.id) {
                continue;
            }
            // A subscription shared by two agents is one account with one plan,
            // and probing it twice would spend two requests to learn one fact.
            if !probed.insert(entry.id.clone()) {
                continue;
            }
            probe_one(entry.id, entry.provider).await;
        }
    }
    info!(
        event = "usage_probe_swept",
        subscriptions = probed.len(),
        "finished one proactive plan usage sweep"
    );
}

/// Probe one subscription, surviving anything it does.
///
/// The work runs in its own task so that a panic below -- in a provider client,
/// a credential parser, or a header reader -- is a logged join error against one
/// subscription instead of the silent death of the whole timer.
async fn probe_one(subscription_id: String, provider: String) {
    let logged_id = subscription_id.clone();
    let logged_provider = provider.clone();
    if let Err(error) = tokio::spawn(probe_subscription(subscription_id, provider)).await {
        warn!(
            event = "usage_probe_panicked",
            subscription = %logged_id,
            provider = %logged_provider,
            %error,
            "a plan usage probe died; the remaining subscriptions are unaffected"
        );
    }
}

async fn probe_subscription(subscription_id: String, provider: String) {
    if usage::is_blocked(&subscription_id) {
        info!(
            event = "usage_probe_skipped_blocked",
            subscription = %subscription_id,
            provider = %provider,
            "a recorded block is still in force; not spending a probe against a \
             rate-limited account"
        );
        return;
    }
    let Some(model) = provider_registry::plan_probe_route(&provider) else {
        info!(
            event = "usage_probe_unroutable",
            subscription = %subscription_id,
            provider = %provider,
            "this provider publishes no plan windows worth a request, or names no \
             model without a catalog call; leaving the row unprobed"
        );
        return;
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
    let response = probe_subscription_usage(&subscription_id, &provider, &request).await;
    let detail = response.error.as_deref();
    usage::record_probe(&subscription_id, &provider, response.success, detail);
    if response.success {
        info!(
            event = "usage_probe_recorded",
            subscription = %subscription_id,
            provider = %provider,
            windows = response.limits.len(),
            "the provider answered a plan usage probe"
        );
        return;
    }
    warn!(
        event = "usage_probe_refused",
        subscription = %subscription_id,
        provider = %provider,
        detail = detail.unwrap_or("the provider gave no reason"),
        "a plan usage probe was refused; the row can now say why instead of \
         reporting an empty plan"
    );
}
