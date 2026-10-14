//! What a credential command came to, as lines a person reads.
//!
//! These moved out of the command file when the enrolment command joined it:
//! the dispatch there is about deciding which operation runs, and the shape
//! of each verdict is its own subject. [`enrolment`] carries the one command
//! whose whole point is that no later run needs a person. A field the
//! gateway left out of a verdict is named as such, never printed as an empty
//! value or a zero count.

pub(crate) mod enrolment;

use serde_json::Value;

use super::text;

/// `field` of `verdict`, or that the verdict did not report it.
fn reported(verdict: &Value, field: &str) -> String {
    match text(verdict, field) {
        Some(value) => value.to_string(),
        None => format!("not reported ({field} is absent from the verdict)"),
    }
}

/// What one refresh came to, as lines.
pub(crate) fn print_refresh(verdict: &Value) {
    println!("provider: {}", reported(verdict, "provider"));
    let attempted = match verdict.get("attempted").and_then(Value::as_u64) {
        Some(attempted) => attempted.to_string(),
        None => "not reported (attempted is absent from the verdict)".to_string(),
    };
    println!("attempted: {attempted}");
    println!("result: {}", reported(verdict, "result"));
    println!("detail: {}", reported(verdict, "detail"));
}

/// What one sign-in came to, as lines.
pub(crate) fn print_sign_in(verdict: &Value) {
    println!("provider: {}", reported(verdict, "provider"));
    println!("login_item: {}", reported(verdict, "login_item"));
    if let Some(account) = text(verdict, "account").filter(|account| !account.is_empty()) {
        println!("account: {account}");
    }
    println!("result: {}", reported(verdict, "result"));
    println!("detail: {}", reported(verdict, "detail"));
}
