//! Which exact account this sign-in drives, and whether it is the account the
//! caller meant.
//!
//! This is separate because it is everything that can be decided before a
//! browser opens, read only off Weles's declaration of the rows it holds. The
//! cost of getting it wrong is not a failed request: it is one real sign-in
//! into a stranger's account, so the refusals here are deliberately more
//! numerous than the successes and each one names the account it protected.

use serde_json::Value;

use super::blocked::Blocked;

/// The selector Weles's health answer must advertise before a named account is
/// asked for. A release without it would silently pick a sign-in row of its
/// own, and the cost of finding that out afterwards is one real sign-in into
/// the wrong account.
pub(super) const LOGIN_ITEM_SELECTOR: &str = "login_item";

/// The exact sign-in row this run will drive.
///
/// A named row must exist and belong to the provider. An unnamed run uses the
/// provider's sole row or the one Weles explicitly marks primary. That primary
/// declaration bootstraps old subscription items which predate `brama:login:`;
/// the successful donation writes the tag, so later renewals no longer need it.
pub(super) fn resolve_login_item(
    health: &Value,
    weles_provider: &str,
    asked: Option<&str>,
) -> Result<String, Blocked> {
    let features = health.get("features").and_then(Value::as_array);
    let advertised = features.is_some_and(|features| {
        features
            .iter()
            .any(|feature| feature.as_str() == Some(LOGIN_ITEM_SELECTOR))
    });
    if !advertised {
        return Err(Blocked::WelesCannotTargetAccount {
            detail: format!(
                "this Weles release does not advertise the {LOGIN_ITEM_SELECTOR} selector, so it \
                 would choose a sign-in row itself; deploy the release that carries it before \
                 signing a named account in"
            ),
        });
    }
    let rows: Vec<&Value> = health
        .get("login_items")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().collect())
        .unwrap_or_default();
    let row_item = |row: &&Value| {
        row.get(LOGIN_ITEM_SELECTOR)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    if let Some(asked) = asked.filter(|asked| !asked.is_empty()) {
        let named: Vec<&&Value> = rows.iter().filter(|row| row_item(row) == asked).collect();
        if named.is_empty() {
            let held = rows
                .iter()
                .map(row_item)
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Blocked::WelesAccountUnknown {
                provider: weles_provider.to_string(),
                detail: format!(
                    "Weles holds no sign-in row for {asked}; it holds {}. That account has to \
                     exist in Weles before it can be signed in",
                    if held.is_empty() { "none".into() } else { held }
                ),
            });
        }
        let providers: Vec<&str> = named
            .iter()
            .filter_map(|row| row.get("provider").and_then(Value::as_str))
            .collect();
        if !providers.is_empty() && providers.iter().all(|held| *held != weles_provider) {
            return Err(Blocked::WelesAccountUnknown {
                provider: weles_provider.to_string(),
                detail: format!(
                    "{asked} is a {} account, not a {weles_provider} one; refusing to sign it in \
                     for the wrong provider",
                    providers[0]
                ),
            });
        }
        return Ok(asked.to_string());
    }
    let matching = rows
        .iter()
        .filter(|row| row.get("provider").and_then(Value::as_str) == Some(weles_provider))
        .collect::<Vec<_>>();
    if matching.len() == 1 {
        return Ok(row_item(matching[0]));
    }
    let primary = matching
        .iter()
        .filter(|row| row.get("primary").and_then(Value::as_bool) == Some(true))
        .collect::<Vec<_>>();
    if primary.len() == 1 {
        return Ok(row_item(primary[0]));
    }
    let held = matching
        .iter()
        .map(|row| row_item(row))
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>();
    if held.is_empty() {
        return Err(Blocked::WelesHoldsNoAccount {
            provider: weles_provider.to_string(),
            detail: format!(
                "Weles holds no sign-in row for provider {weles_provider}; that account has to \
                 exist in Weles before it can be signed in"
            ),
        });
    }
    Err(Blocked::WelesAccountAmbiguous {
        provider: weles_provider.to_string(),
        detail: format!(
            "Weles holds {} sign-in rows for provider {weles_provider} ({}) and marks {} primary; \
             it must declare exactly one primary account before an unmapped subscription can renew",
            held.len(),
            held.join(", "),
            primary.len()
        ),
    })
}

/// Whether the row about to be signed in renews a different subscription than
/// the one the caller named, stated as the sentence the operator reads.
///
/// A row Weles declares for another subscription is the case worth refusing
/// loudly. A row Weles declares nothing about is only refused when this run
/// chose the account itself: a caller who named the row already asserted the
/// mapping, while an inferred primary plus a silent declaration is a guess.
pub(super) fn subscription_mismatch(
    health: &Value,
    login_item: &str,
    expected: &str,
    selecting_declared_primary: bool,
) -> Option<String> {
    let declared_subscription = health
        .get("login_items")
        .and_then(Value::as_array)
        .and_then(|rows| {
            rows.iter().find(|row| {
                row.get(LOGIN_ITEM_SELECTOR).and_then(Value::as_str) == Some(login_item)
            })
        })
        .and_then(|row| row.get("subscription_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty());
    match declared_subscription {
        Some(declared) if expected == declared => None,
        Some(declared) => Some(format!(
            "Weles declares {login_item} for subscription {declared}, not {expected}; \
             refusing to renew the wrong account"
        )),
        None if !selecting_declared_primary => None,
        None => Some(format!(
            "Weles does not declare which subscription {login_item} renews; refusing \
             to infer the account for {expected}"
        )),
    }
}
