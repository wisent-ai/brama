//! What Weles's reauth answer says about the browser trajectory it drove.
//!
//! This is separate because reading the answer is the one place where the
//! sign-in stops being Brama's decision and becomes a report about someone
//! else's run. Two questions are asked of it: whose account came back, and
//! whether the run Weles finished is a run this request may claim. A failed
//! trajectory used to return its exact stderr here and Brama replaced it with
//! an attribution sentence that hid the cause, so the sentence this module
//! builds carries the run, the status, the exit code and Weles's own words.

use serde_json::Value;

use super::account::LOGIN_ITEM_SELECTOR;

/// Why this reauth answer is not a sign-in of the row that was asked for, or
/// `None` when it is.
///
/// Echoing the row proves attribution; `ok` proves the browser trajectory
/// finished. Both are required, because an answer about another account is as
/// useless as a trajectory that never reached the consent screen.
pub(super) fn refusal(answer: &Value, status: u16, login_item: &str) -> Option<String> {
    let echoed = answer.get(LOGIN_ITEM_SELECTOR).and_then(Value::as_str);
    let attributed = echoed == Some(login_item);
    let succeeded = status == 200 && answer.get("ok").and_then(Value::as_bool) == Some(true);
    if attributed && succeeded {
        return None;
    }
    let run_id = answer
        .get("run_id")
        .and_then(Value::as_str)
        .unwrap_or("unreported");
    let exit_code = answer
        .get("exitCode")
        .and_then(Value::as_i64)
        .map(|code| code.to_string())
        .unwrap_or_else(|| "unreported".to_string());
    let timed_out = answer
        .get("timed_out")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let said = ["error", "message", "stderr_tail", "stdout_tail"]
        .iter()
        .filter_map(|field| answer.get(field).and_then(Value::as_str))
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    let said: String = said.chars().take(1800).collect();
    Some(format!(
        "Weles sign-in run {run_id} answered HTTP {status}, exit {exit_code}, timed_out={timed_out}, \
         login_item={}; {}",
        echoed.unwrap_or("unreported"),
        if said.is_empty() {
            "the trajectory reported no stderr or message"
        } else {
            said.as_str()
        }
    ))
}
