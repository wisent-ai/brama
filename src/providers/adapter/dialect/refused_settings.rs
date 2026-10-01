//! Request settings a provider has told Brama a model refuses.
//!
//! Anthropic's newest models answer a request that carries `temperature`
//! with HTTP 400 and the sentence "`temperature` is deprecated for this
//! model", whatever the value. The gateway invents no temperature of its
//! own, and when a caller sends one, every Claude subscription model refuses
//! in turn and the pool would report "no working subscription model for
//! signed agent" for a fleet whose credentials are fine. A model list would
//! go stale with the next
//! release; the provider's own refusal is exact, so Brama learns from it:
//! the model is noted once, the request is sent again without the setting,
//! and every later request to that model omits it for the life of the
//! process.

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};

use reqwest::StatusCode;

static TEMPERATURE_REFUSED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// The provider's sentence for a model that takes no temperature.
const TEMPERATURE_DEPRECATED: &str = "`temperature` is deprecated for this model";

/// Whether requests to this model must leave `temperature` out.
pub(in crate::providers::adapter) fn omits_temperature(model_id: &str) -> bool {
    TEMPERATURE_REFUSED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(model_id)
}

/// Reads a provider refusal; true when it just taught Brama that this
/// model refuses `temperature`, which is the one case worth one more send.
pub(in crate::providers::adapter) fn learn_refused_temperature(
    model_id: &str,
    status: StatusCode,
    body: &str,
) -> bool {
    if status != StatusCode::BAD_REQUEST || !body.contains(TEMPERATURE_DEPRECATED) {
        return false;
    }
    let learned = TEMPERATURE_REFUSED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(model_id.to_owned());
    if learned {
        tracing::warn!(
            model = model_id,
            "provider refuses `temperature` for this model; later requests omit it"
        );
    }
    learned
}
