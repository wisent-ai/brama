//! Which account an agent's consecutive turns keep landing on, and for how
//! long that preference outlives the call that set it.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::gateway::broker;
use crate::subscription_dispatch::usage;

/// One agent's pinned credential for one provider, process-local.
///
/// A pin is the dispatcher's memory of which account served this agent most
/// recently, held so consecutive requests land on the same subscription until
/// its own plan window ends. That is what makes the provider's prompt cache
/// reachable at all -- a cache entry lives behind one account, and the old
/// shuffle scattered an agent's turns across all of them -- and it keeps one
/// turn's spend in one account's ledger instead of smearing it across the
/// pool. The pin is a preference, never a grant: it is consulted after
/// eligibility, skipped when the credential is blocked, retired, or reporting
/// a full window, and it never outlives the window it was read from. It is
/// not persisted, because its whole meaning expires with the window anyway
/// and a restart simply shuffles once.
struct Pin {
    credential_id: String,
    expires_at_ms: i64,
}

/// How long a pin survives when the credential's own windows name no reset.
///
/// Five hours matches the shortest plan window any provider on this fleet
/// publishes, so the stand-in cannot outlive the thing it approximates. The
/// cap exists because a provider's reset instant is trusted only within a
/// day: past that, an error in one header would hold an agent on one account
/// for longer than any real window lasts.
const DEFAULT_PIN_MS: i64 = 5 * 60 * 60 * 1_000;
const MAX_PIN_MS: i64 = 24 * 60 * 60 * 1_000;

static PINS: LazyLock<Mutex<HashMap<(String, String), Pin>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// The credential this agent is pinned to for this provider, if the pin is
/// still inside its own window.
fn pinned_credential(agent_id: &str, provider: &str) -> Option<String> {
    let key = (agent_id.to_string(), provider.to_string());
    let mut pins = PINS.lock().ok()?;
    let pin = pins.get(&key)?;
    if pin.expires_at_ms <= now_ms() {
        pins.remove(&key);
        return None;
    }
    Some(pin.credential_id.clone())
}

/// Pin this agent to the credential that just served it, until that
/// credential's tightest window says otherwise.
pub(in crate::subscription_dispatch::dispatch) fn pin_credential(
    agent_id: &str,
    provider: &str,
    credential_id: &str,
) {
    let now = now_ms();
    let expires_at_ms = usage::next_reset_ms(credential_id)
        .unwrap_or_else(|| now.saturating_add(DEFAULT_PIN_MS))
        .min(now.saturating_add(MAX_PIN_MS));
    if let Ok(mut pins) = PINS.lock() {
        pins.insert(
            (agent_id.to_string(), provider.to_string()),
            Pin {
                credential_id: credential_id.to_string(),
                expires_at_ms,
            },
        );
    }
}

/// Move the pinned credential to the front of the candidate list.
///
/// Reorder only: the pin never makes an ineligible credential eligible, and
/// an explicit billing target has already reduced the list to one row, so it
/// is untouched. A pinned credential reporting a full window is not
/// promoted -- the provider's own numbers say the next call there is the one
/// that buys the 429, and the block that answer writes is what the pin exists
/// to avoid paying for.
pub(in crate::subscription_dispatch::dispatch) fn apply_pin(
    rows: &mut [broker::SubscriptionEntry],
    agent_id: &str,
    provider: &str,
) {
    let Some(pinned) = pinned_credential(agent_id, provider) else {
        return;
    };
    if usage::used_fraction(&pinned).is_some_and(|fraction| fraction >= 1.0) {
        return;
    }
    if let Some(position) = rows.iter().position(|entry| entry.id == pinned) {
        rows.swap(0, position);
    }
}
