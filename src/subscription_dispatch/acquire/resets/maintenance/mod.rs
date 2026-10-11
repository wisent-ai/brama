//! Read saved capacity on each maintenance pass before deciding to pay for more.
//! A provider the operator stated a saved-reset policy for is redeemed under
//! it; every other provider's offers are only observed.

use crate::gateway::broker;
use serde_json::{json, Value};

pub async fn run() -> Value {
    let members = match broker::list_all_subscriptions().await {
        Ok(members) => members,
        Err(error) => return json!({"ok": false, "error": error, "members": []}),
    };
    let stated = match super::settings::all() {
        Ok(stated) => stated,
        Err(error) => return json!({"ok": false, "error": error, "members": []}),
    };
    let mut results = Vec::new();
    let mut ok = true;
    for member in members {
        match crate::journal::is_retired(&member.id) {
            Ok(true) => continue,
            Ok(false) => {}
            // Without the journal no member is known not to be retired, so
            // nothing is redeemed this pass and the cause is the answer.
            Err(error) => return json!({"ok": false, "error": error, "members": results}),
        }
        // A member whose provider is not declared cannot be maintained; it is
        // reported with the refusal, not passed over as if it had no resets.
        let declared = match super::super::declaration::provider(&member.provider) {
            Ok(provider) => provider,
            Err(error) => {
                ok = false;
                results.push(
                    json!({"member": member.id, "provider": member.provider, "error": error}),
                );
                continue;
            }
        };
        if declared.resets.is_none() {
            continue;
        }
        let result = if stated.contains_key(&member.provider) {
            super::redeem::run(
                &member.provider,
                &member.id,
                "maintenance: restore exhausted or expiring saved capacity before purchase",
                true,
            )
            .await
        } else {
            super::refresh(&member.id, &member.provider)
                .await
                .map(|offer| json!({"ok": true, "result": "observed", "offer": offer}))
        };
        match result {
            Ok(report) => {
                ok &= report["ok"].as_bool() == Some(true);
                results.push(
                    json!({"member": member.id, "provider": member.provider, "report": report}),
                );
            }
            Err(error) => {
                ok = false;
                results.push(
                    json!({"member": member.id, "provider": member.provider, "error": error}),
                );
            }
        }
    }
    json!({"ok": ok, "members": results})
}
