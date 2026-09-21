//! `brama subscription enrol-authenticator`: the repair that makes every
//! later sign-in unattended.
//!
//! An automatic sign-in stops at `google_2fa_material_missing` when the
//! Skarbiec login behind a subscription holds no authenticator seed, and no
//! sign-in can produce one, because the provider shows a setup key only
//! inside a signed-in session. This orders the Weles trajectory that signs in
//! once, enrols an authenticator, reads the key it is shown and writes it to
//! that login item. It takes one approval on the operator's phone, and after
//! it no sign-in for that account needs a person again — which is the whole
//! difference between this and pasting a one-time code.

use brama::subscription_dispatch::sign_in::{enrol_authenticator as enrol, weles_provider};
use serde_json::json;

pub(crate) async fn enrol_authenticator(
    provider: &str,
    subscription_id: &str,
    reason: &str,
    login_item: Option<&str>,
    login_timeout_ms: u64,
    as_json: bool,
) {
    let Some(weles) = weles_provider(provider) else {
        eprintln!(
            "provider `{provider}` has no Weles sign-in, so it has no authenticator to enrol"
        );
        std::process::exit(1);
    };
    match enrol(
        provider,
        weles,
        subscription_id,
        login_item,
        login_timeout_ms,
    )
    .await
    {
        Ok(enrolment) => {
            if as_json {
                crate::cli::print_json(&json!({
                    "provider": provider,
                    "subscription_id": enrolment.subscription_id,
                    "login_item": enrolment.login_item,
                    "run_id": enrolment.run_id,
                    "seed_present": enrolment.seed_present,
                    "reason": reason,
                    "result": if enrolment.ok() { "enrolled" } else { "not_enrolled" },
                    "detail": enrolment.detail,
                }));
            } else {
                println!("provider: {provider}");
                println!("subscription_id: {}", enrolment.subscription_id);
                println!("login_item: {}", enrolment.login_item);
                println!("run: {}", enrolment.run_id);
                println!(
                    "result: {}",
                    if enrolment.ok() {
                        "enrolled"
                    } else {
                        "not_enrolled"
                    }
                );
                println!("detail: {}", enrolment.detail);
            }
            // The seed either answers the provider's second factor from now
            // on or it does not, and the caller is repairing a subscription
            // that cannot sign itself in: the status says which.
            if !enrolment.ok() {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
