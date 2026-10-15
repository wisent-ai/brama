//! Brama requests authentication for a Skarbiec subscription; Weles executes it.
mod attempt;
pub mod blocked;
pub mod manual;
mod recovery;
mod trajectory;
mod verdict;
pub mod worker;

use attempt::execute;
pub use blocked::{Blocked, SignInError};
pub use recovery::membership::admit_recoverable;
use serde_json::{json, Value};
use verdict::{verdict, FAILED};
pub use worker::enrolment::{enrol_authenticator, Enrolment};
pub use worker::progress::{sentence as progress_sentence, Progress};

pub struct SignInOptions {
    pub provider: String,
    /// Optional exact Skarbiec login reference for an explicitly selected run.
    pub login_item: Option<String>,
    pub subscription_id: Option<String>,
    pub reason: String,
    /// Receives each event Weles reports while the sign-in runs, so the
    /// caller can say what the run is doing and whom it waits for.
    pub progress: Option<Progress>,
}

pub use verdict::{automatic_sign_in_sentence, observed_failure};
pub use worker::weles_provider;

pub async fn sign_in_provider(options: SignInOptions) -> Result<Value, SignInError> {
    let result = recovery::execute(&options).await;
    if let Err(error) = &result {
        // Only an exchange Weles actually answered is journaled. A refusal
        // taken before the worker was reached -- an unknown provider, a
        // missing startup credential, a worker that never answered, a vault
        // that names no subscription -- records nothing, because a verdict
        // here would put an attempt in the journal that was never made, and
        // an operator reading the journal during an incident would find a
        // sign-in that "failed" against a provider nothing was asked of.
        let Some(Blocked::Operation {
            code,
            stage,
            status: Some(status),
            ..
        }) = error.blocked()
        else {
            return result;
        };
        verdict(
            &options,
            &Value::Null,
            FAILED,
            error.to_string(),
            json!({
                "code": code, "stage": stage, "http_status": status,
                "browser_started": false, "retryable": true,
            }),
            Value::Null,
        );
    }
    result
}
