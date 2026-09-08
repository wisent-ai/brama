//! The one shape a sign-in verdict takes, for the operator and for the audit
//! record alike.
//!
//! This is separate because every exit from a sign-in -- an account refused
//! before a browser opened, a trajectory Weles could not finish, a confirmed
//! sign-in whose refresh obtained a credential -- leaves through this one
//! function, and the two words the caller's exit status is read off are fixed
//! beside it. Printing and recording happen in the same place on purpose: a
//! record cannot say something the operator was never told.

use serde_json::{json, Value};

/// A run whose sign-in was confirmed and whose follow-up refresh obtained a
/// credential, and every other run. The caller's exit status is read off this,
/// so the two words are fixed here rather than spelled at each return.
pub(super) const SIGNED_IN: &str = "signed_in";
pub(super) const FAILED: &str = "failed";

/// One verdict, in the shape the caller prints and the audit record keeps.
/// Both are written here so a record cannot say something the operator was
/// never told.
#[allow(clippy::too_many_arguments)]
pub(super) fn verdict(
    subscription_id: Option<&str>,
    provider: &str,
    login_item: &str,
    reason: &str,
    result: &str,
    http_status: u16,
    account: &str,
    detail: String,
    refresh: Value,
) -> Value {
    crate::journal::record_subscription_sign_in(
        subscription_id,
        provider,
        login_item,
        reason,
        result,
        &detail,
    );
    json!({
        "subscription_id": subscription_id,
        "provider": provider,
        "login_item": login_item,
        "account": account,
        "result": result,
        "http_status": http_status,
        "detail": detail,
        "refresh": refresh,
    })
}
