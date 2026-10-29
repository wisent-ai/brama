//! The sign-in failure a subscription member is held by now, read from its
//! automatic sign-in check and the journal's last sign-in record.

use serde_json::Value;

use super::super::blocked::{journal_unreadable, observed_at};
use super::super::Blocked;
use super::FAILED;

/// The failure that holds member `id`, or none when nothing does: its
/// automatic sign-in check failed and nothing signed it in since, or its last
/// recorded sign-in failed and no active credential was recorded after it.
/// A journal that cannot be read, or a failed record that does not state
/// what refused it, holds the member with that cause rather than calling it
/// clear or inventing the refusal.
pub fn observed_failure(id: &str) -> Option<Blocked> {
    let current = crate::subscription_dispatch::usage::usage_for(id);
    if let Some(check) = current
        .as_ref()
        .and_then(|entry| entry.sign_in_check_failure.as_ref())
    {
        let active_after_check = current
            .as_ref()
            .and_then(|entry| entry.credential.as_ref())
            .is_some_and(|credential| {
                credential.state == crate::subscription_dispatch::usage::CredentialState::Active
                    && credential.recorded_at_ms >= check.at_ms
            });
        let completed_after_check = match crate::journal::latest_subscription_sign_in(id) {
            Ok(latest) => latest
                .and_then(|attempt| attempt.get("at_ms").and_then(Value::as_i64))
                .is_some_and(|at| at >= check.at_ms),
            Err(cause) => return Some(journal_unreadable(cause)),
        };
        if !active_after_check && !completed_after_check {
            return Some(Blocked::Operation {
                code: check.code.clone(),
                stage: "automatic_sign_in_check".into(),
                detail: format!(
                    "{} ({})",
                    check.detail,
                    observed_at(Some(check.at_ms), "checked")
                ),
                status: None,
            });
        }
    }
    let previous = match crate::journal::latest_subscription_sign_in(id) {
        Ok(previous) => previous?,
        // Whether the last sign-in failed lives in the journal; unreadable,
        // the member is held with the journal's cause rather than called clear.
        Err(cause) => return Some(journal_unreadable(cause)),
    };
    if previous.get("result").and_then(Value::as_str) != Some(FAILED) {
        return None;
    }
    // An attempt refused before any run (identity, login lookup) records
    // `started_at_ms` as null, not absent: read it as a number, so the
    // completion instant stands in for a run start that never happened.
    let began = previous
        .get("started_at_ms")
        .and_then(Value::as_i64)
        .or_else(|| previous.get("at_ms").and_then(Value::as_i64));
    if let (Some(began), Some(credential)) = (
        began,
        current.as_ref().and_then(|usage| usage.credential.as_ref()),
    ) {
        if credential.state == crate::subscription_dispatch::usage::CredentialState::Active
            && credential.recorded_at_ms >= began
        {
            return None;
        }
    }
    let recorded = |pointer: &str| previous.pointer(pointer).and_then(Value::as_str);
    let status = match previous
        .pointer("/failure/http_status")
        .and_then(Value::as_u64)
    {
        Some(value) => match u16::try_from(value) {
            Ok(status) => Some(status),
            Err(error) => {
                return Some(journal_unreadable(format!(
                    "the last recorded sign-in of {id} names HTTP status {value}: {error}"
                )))
            }
        },
        None => None,
    };
    // A failed record states its failure's code and stage; one that does not
    // cannot say what refused the sign-in, so the member is held with the
    // record named as unreadable rather than with a code nobody recorded.
    let (Some(code), Some(stage)) = (recorded("/failure/code"), recorded("/failure/stage")) else {
        return Some(journal_unreadable(format!(
            "the last recorded sign-in of {id} failed ({}) without stating its failure code ({}) and stage ({}), so what refused it is unknown",
            observed_at(began, "attempted"),
            stated(recorded("/failure/code")),
            stated(recorded("/failure/stage")),
        )));
    };
    Some(Blocked::Operation {
        code: code.into(),
        stage: stage.into(),
        detail: format!(
            "{} ({})",
            match recorded("/detail") {
                Some(detail) => detail,
                None => "the recorded sign-in states no detail",
            },
            observed_at(began, "attempted")
        ),
        status,
    })
}

/// Whether a record states a field, as a refusal names it.
fn stated(field: Option<&str>) -> &'static str {
    match field {
        Some(_) => "stated",
        None => "absent",
    }
}
