//! An age the operator declares in the gateway's environment, in whole
//! seconds. Brama assumes none: an unset variable is an undeclared age, and
//! every caller says what that means for it.

use std::time::{Duration, Instant};

/// The age `variable` declares, `None` when it is unset, refused by name when
/// it is not a whole number of seconds.
pub fn declared_age(variable: &str) -> Result<Option<Duration>, String> {
    match std::env::var(variable) {
        Err(_) => Ok(None),
        Ok(value) => value
            .trim()
            .parse()
            .map(|seconds| Some(Duration::from_secs(seconds)))
            .map_err(|_| format!("{variable} must be a whole number of seconds; it is {value:?}")),
    }
}

/// Whether something read at `read_at` may still be served under the age
/// `variable` declares. Undeclared, nothing read earlier is served: the next
/// caller reads again. A refused declaration is logged and serves nothing.
pub fn still_fresh(variable: &str, read_at: Instant) -> bool {
    match declared_age(variable) {
        Ok(age) => age.is_some_and(|age| read_at.elapsed() < age),
        Err(refusal) => {
            tracing::warn!(target: "brama::declared_age", "{refusal}");
            false
        }
    }
}
