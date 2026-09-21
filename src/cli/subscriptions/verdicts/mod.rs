//! What a credential command came to, as lines a person reads.
//!
//! These moved out of the command file when the enrolment command joined it:
//! the dispatch there is about deciding which operation runs, and the shape
//! of each verdict is its own subject. [`enrolment`] carries the one command
//! whose whole point is that no later run needs a person.

pub(crate) mod enrolment;

use serde_json::Value;

use super::text;

/// What one refresh came to, as lines.
pub(crate) fn print_refresh(verdict: &Value) {
    println!(
        "provider: {}",
        text(verdict, "provider").unwrap_or_default()
    );
    println!(
        "attempted: {}",
        verdict
            .get("attempted")
            .and_then(Value::as_u64)
            .unwrap_or_default()
    );
    println!("result: {}", text(verdict, "result").unwrap_or_default());
    println!("detail: {}", text(verdict, "detail").unwrap_or_default());
}

/// What one sign-in came to, as lines.
pub(crate) fn print_sign_in(verdict: &Value) {
    println!(
        "provider: {}",
        text(verdict, "provider").unwrap_or_default()
    );
    println!(
        "login_item: {}",
        text(verdict, "login_item").unwrap_or_default()
    );
    if let Some(account) = text(verdict, "account").filter(|account| !account.is_empty()) {
        println!("account: {account}");
    }
    println!("result: {}", text(verdict, "result").unwrap_or_default());
    println!("detail: {}", text(verdict, "detail").unwrap_or_default());
}
