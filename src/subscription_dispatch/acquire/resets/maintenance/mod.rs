//! Read saved capacity on each maintenance pass before deciding to pay for more.

use crate::gateway::broker;
use serde_json::{json, Value};

pub async fn run() -> Value {
    let members = match broker::list_all_subscriptions().await {
        Ok(members) => members,
        Err(error) => return json!({"ok": false, "error": error, "members": []}),
    };
    let mut results = Vec::new();
    let mut ok = true;
    for member in members {
        if crate::journal::is_retired(&member.id) {
            continue;
        }
        let declared = match super::super::declaration::provider(&member.provider) {
            Ok(provider) => provider,
            Err(_) => continue,
        };
        let Some(policy) = &declared.resets else {
            continue;
        };
        let result = if policy.auto_redeem {
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
