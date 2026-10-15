//! What Weles's reauth answer says about the browser trajectory it drove.
//!
//! This is separate because reading the answer is the one place where the
//! sign-in stops being Brama's decision and becomes a report about someone
//! else's run. Two questions are asked of it: whose account came back, and
//! whether the run Weles finished is a run this request may claim. A refusal
//! carries the run, the status, the exit code and Weles's own words, so the
//! cause of a failed trajectory reaches whoever reads it.

use serde_json::Value;

use super::worker::account::{HTTP_OK, LOGIN_ITEM_SELECTOR};

/// A field of Weles's answer as text, or the statement that the answer
/// carried none: the refusal names what is missing instead of omitting it.
fn reported(value: Option<String>) -> String {
    match value {
        Some(value) => value,
        None => "unreported".to_string(),
    }
}

/// Why this reauth answer is not a sign-in of the row that was asked for, or
/// `None` when it is.
///
/// Echoing the row proves attribution; `ok` proves the browser trajectory
/// finished. Both are required, because an answer about another account is as
/// useless as a trajectory that never reached the consent screen.
pub(super) fn refusal(answer: &Value, status: u16, login_item: &str) -> Option<String> {
    let echoed = answer.get(LOGIN_ITEM_SELECTOR).and_then(Value::as_str);
    let attributed = echoed == Some(login_item);
    let succeeded = status == HTTP_OK && answer.get("ok").and_then(Value::as_bool) == Some(true);
    if attributed && succeeded {
        return None;
    }
    let run_id = reported(
        answer
            .get("run_id")
            .and_then(Value::as_str)
            .map(str::to_string),
    );
    let exit_code = reported(
        answer
            .get("exitCode")
            .and_then(Value::as_i64)
            .map(|code| code.to_string()),
    );
    let echoed = reported(echoed.map(str::to_string));
    let said = ["error", "message", "stderr_tail", "stdout_tail"]
        .iter()
        .filter_map(|field| answer.get(field).and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    Some(format!(
        "Weles sign-in run {run_id} answered HTTP {status}, exit {exit_code}, login_item={echoed}; {}",
        if said.is_empty() {
            "the trajectory reported no stderr or message"
        } else {
            said.as_str()
        }
    ))
}
