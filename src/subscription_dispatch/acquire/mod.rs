//! Buying a new subscription account when the pool cannot serve, up to the
//! number of accounts the operator allows.
//!
//! Every provider is described once, in `providers.json` (`declaration`):
//! the Weles provider whose purchase trajectory buys an account, the names of
//! the operator's account cap and sessions-per-subscription limit in
//! `numeric-provenance.json`, and how each harness takes the provider's
//! accounts. Nothing else here names a provider; a provider without a
//! declaration, or a declaration missing a piece, is refused by that piece.
//!
//! An acquisition is decided on read facts only (`decide`): the accounts the
//! vault lists for the provider, each one's plan read fresh from the
//! provider, the leases its subscriptions carry and the operator's cap. It
//! buys for one of three shortages — every account has spent its plan, every
//! usable subscription carries the operator's limit of sessions, or no
//! account can be used at all — while the pool holds fewer accounts than the
//! cap; every other outcome is a refusal that names the account or the
//! number that stopped it. The plan bought is the plan the pool's accounts
//! hold, as the provider states it.
//!
//! Weles does the purchase, Brama proves the new grant with a refresh exactly
//! as a sign-in is proved (`buy`), and every attempt is journaled when it is
//! requested and when it ends. A `brama maintain` pass runs the same
//! decision for every declared provider; after an automatic attempt that
//! failed or never answered, the pass stops buying until an operator runs the
//! acquisition again, because each attempt can spend money and the same
//! failure repeated every pass would spend it again.

mod decide;
pub mod declaration;
pub mod hand_over;
mod purchase;
mod weles;

use std::sync::LazyLock;

use serde_json::{json, Value};
use tracing::{info, warn};

use crate::subscription_dispatch::sign_in::worker::progress::Progress;

/// A new account was bought and its grant proved.
pub const ACQUIRED: &str = "acquired";
/// The decision not to buy, with the fact that stopped it.
pub const REFUSED: &str = "refused";
/// A purchase was asked of Weles and did not end in a proved grant.
pub const FAILED: &str = "failed";
/// Journaled when Weles is asked, before its answer.
const REQUESTED: &str = "requested";

/// Who asked for the acquisition. An operator's request runs even after a
/// failed attempt; a maintenance pass does not. A lease refused because every
/// usable subscription carries the operator's limit of sessions buys for
/// that shortage: the plans are not spent, the accounts are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Operator,
    Maintenance,
    SessionsFull,
}

impl Trigger {
    fn name(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::Maintenance => "maintenance",
            Self::SessionsFull => "sessions_full",
        }
    }
}

pub struct AcquireOptions {
    pub provider: String,
    pub reason: String,
    pub trigger: Trigger,
    /// Receives every event Weles reports while the purchase runs.
    pub progress: Option<Progress>,
}

/// One acquisition at a time per process: two passes that both saw the pool
/// spent would otherwise buy two accounts for one shortage.
static IN_FLIGHT: LazyLock<std::sync::Arc<tokio::sync::Mutex<()>>> =
    LazyLock::new(|| std::sync::Arc::new(tokio::sync::Mutex::new(())));

/// Every declared provider, in name order: the providers a maintenance pass
/// decides for.
pub fn acquirable_providers() -> Vec<&'static str> {
    declaration::declared_providers()
}

/// Decide, and when every condition holds, buy one account and prove it.
///
/// `Err` is a request that cannot be decided at all (an unknown provider, no
/// reason); every decided outcome, a refusal included, is `Ok` with a verdict
/// whose `result` is [`ACQUIRED`], [`REFUSED`] or [`FAILED`].
pub async fn acquire_account(options: AcquireOptions) -> Result<Value, String> {
    validate(&options)?;
    let Ok(_flight) = IN_FLIGHT.try_lock() else {
        return Ok(decide::refusal(
            &options,
            "acquisition_running",
            format!(
                "an acquisition of a {} account is already running in this process; its verdict \
                 is journaled when it ends",
                options.provider
            ),
            json!({}),
        ));
    };
    match decide::decide(&options).await {
        Ok(shortage) => Ok(purchase::buy(&options, shortage).await),
        Err(verdict) => Ok(*verdict),
    }
}

/// Decide for a maintenance pass and, when every condition holds, start the
/// purchase without holding the pass for the length of a browser run. The
/// answer is the decision; the purchase's own verdict is journaled and logged
/// when it ends.
pub async fn maintenance_pass() -> Value {
    let mut verdicts = Vec::new();
    for provider in acquirable_providers() {
        let options = AcquireOptions {
            provider: provider.to_owned(),
            reason: format!("no {provider} account in the pool can serve: each has spent its plan or cannot be used"),
            trigger: Trigger::Maintenance,
            progress: Some(std::sync::Arc::new(move |event: &Value| {
                if let Some(sentence) =
                    crate::subscription_dispatch::sign_in::progress_sentence(event)
                {
                    info!(event = "subscription_acquisition_progress", provider, %sentence);
                }
            })),
        };
        let Ok(flight) = std::sync::Arc::clone(&IN_FLIGHT).try_lock_owned() else {
            verdicts.push(decide::refusal(
                &options,
                "acquisition_running",
                "an acquisition started by an earlier pass is still running".to_string(),
                json!({}),
            ));
            continue;
        };
        match decide::decide(&options).await {
            Err(verdict) => verdicts.push(*verdict),
            Ok(shortage) => {
                verdicts.push(json!({
                    "provider": provider, "result": "started", "code": "purchase_started",
                    "detail": format!(
                        "{}, and the operator allows {}; Weles is buying one on plan {}, and the \
                         verdict is journaled and logged when it ends",
                        shortage.why, shortage.cap, shortage.plan_tier
                    ),
                    "trigger": options.trigger.name(), "shortage": shortage.kind,
                    "cap": shortage.cap, "accounts": shortage.accounts,
                    "standings": shortage.standings, "plan_tier": shortage.plan_tier,
                }));
                // The slot travels with the purchase, so no later pass can
                // decide on the same shortage while this one is buying.
                tokio::spawn(async move {
                    let _flight = flight;
                    let verdict = purchase::buy(&options, shortage).await;
                    let result = verdict["result"].clone();
                    if result == json!(ACQUIRED) {
                        info!(event = "subscription_acquisition_finished", provider = %options.provider,
                            %result, detail = %verdict["detail"]);
                    } else {
                        warn!(event = "subscription_acquisition_finished", provider = %options.provider,
                            %result, detail = %verdict["detail"]);
                    }
                });
            }
        }
    }
    json!({ "verdicts": verdicts })
}

fn validate(options: &AcquireOptions) -> Result<(), String> {
    declaration::provider(&options.provider)?;
    if options.reason.trim().is_empty() {
        return Err("--reason must say why an account is being bought".to_string());
    }
    Ok(())
}
