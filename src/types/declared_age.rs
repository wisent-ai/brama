//! An age the operator declares in the gateway's environment, read through
//! `crate::environment`. An age is a whole number of seconds; Brama assumes
//! none, and every caller says what an undeclared one means for it.

use std::time::{Duration, Instant};

use crate::environment::declared_text;

/// The age `variable` declares, `None` when it is unset, refused by name when
/// it is not a whole number of seconds.
pub fn declared_age(variable: &str) -> Result<Option<Duration>, String> {
    let Some(value) = declared_text(variable)? else {
        return Ok(None);
    };
    value
        .trim()
        .parse()
        .map(|seconds| Some(Duration::from_secs(seconds)))
        .map_err(|error| {
            format!("{variable} must be a whole number of seconds; it is {value:?}: {error}")
        })
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
