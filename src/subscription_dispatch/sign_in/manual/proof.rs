//! The half of a manual sign-in that proves it: exchange the paste, store the
//! grant, then make the provider renew what it just issued.
//!
//! The refresh is not decoration. A grant that was stored and never exercised
//! is the state the pool was in on 2026-09-12 - bytes in the vault, no
//! credential - and the only proof a sign-in worked is the provider renewing
//! the grant through the same path the sweep will use from now on. The CLI on
//! a terminal and the gateway answering Brama Desktop both end here, so the
//! two cannot disagree about what "signed in" means.

use super::{complete, AuthorizationRequest, ManualSignIn};
use crate::subscription_dispatch::pool::refresh_subscription;

/// Exchange, store and prove. `reason` is what the refresh journals.
pub async fn finish(
    request: AuthorizationRequest,
    pasted: &str,
    reason: &str,
) -> Result<ManualSignIn, String> {
    if reason.trim().is_empty() {
        return Err("--reason must say why this sign-in is being run".into());
    }
    let provider = request.provider().to_owned();
    let subscription_id = request.subscription_id().to_owned();
    let stored = complete(request, pasted).await?;
    if stored.result != "signed_in" {
        return Ok(stored);
    }
    let refresh = refresh_subscription(&provider, &subscription_id, reason)
        .await
        .map_err(|detail| {
            format!("the grant is stored, but the refresh that proves it failed: {detail}")
        })?;
    if refresh.get("result").and_then(|value| value.as_str()) == Some("refreshed") {
        return Ok(stored);
    }
    Ok(ManualSignIn {
        provider: stored.provider,
        subscription_id: stored.subscription_id,
        account: stored.account,
        result: "failed",
        detail: format!(
            "the grant is stored, but the provider would not renew it: {}",
            refresh
                .get("detail")
                .and_then(|value| value.as_str())
                .unwrap_or("refresh returned no reason")
        ),
    })
}
