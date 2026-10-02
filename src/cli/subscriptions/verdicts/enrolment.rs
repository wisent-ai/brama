//! Enrol authenticator material for a resolved Google login without replacing
//! an existing authenticator. Google may require a phone approval. Success
//! requires both a confirmed Weles run and a usable seed reported by Skarbiec;
//! it does not promise that Google will never request another approval.

use brama::subscription_dispatch::sign_in::{enrol_authenticator as enrol, weles_provider};
use serde_json::json;

pub(crate) async fn enrol_authenticator(
    provider: &str,
    subscription_id: &str,
    reason: &str,
    login_item: Option<&str>,
    as_json: bool,
) {
    let Some(weles) = weles_provider(provider) else {
        eprintln!(
            "provider `{provider}` has no Weles sign-in, so it has no authenticator to enrol"
        );
        std::process::exit(1);
    };
    match enrol(provider, weles, subscription_id, login_item).await {
        Ok(enrolment) => {
            if as_json {
                crate::cli::print_json(&json!({
                    "provider": provider,
                    "subscription_id": enrolment.subscription_id,
                    "login_item": enrolment.login_item,
                    "run_id": enrolment.run_id,
                    "seed_present": enrolment.seed_present,
                    "confirmed": enrolment.confirmed,
                    "reason": reason,
                    "result": if enrolment.ok() { "enrolled" } else { "not_enrolled" },
                    "detail": enrolment.detail,
                }));
            } else {
                println!("provider: {provider}");
                println!("subscription_id: {}", enrolment.subscription_id);
                println!("login_item: {}", enrolment.login_item);
                println!("run: {}", enrolment.run_id);
                println!("confirmed: {}", enrolment.confirmed);
                println!("seed_present: {}", enrolment.seed_present);
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
