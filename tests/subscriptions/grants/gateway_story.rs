//! Two stories about handing a grant on: one refused before anything is
//! stored, and the desktop path end to end through a gateway elsewhere.

use super::*;

#[test]
fn a_document_without_a_refresh_token_is_refused_before_anything_is_stored() {
    let gateway = Gateway::start("grant-unrenewable", &[(AGENT, "codex", "pool-agent")]);
    let (status, answer) = gateway.console(
        GRANT,
        Method::POST,
        Some(
            &json!({"subscription_id": "pool-agent", "reason": "story", "harness": "codex",
                     "document": json!({"tokens": {"access_token": "sk-oat-only"}}).to_string()}),
        ),
    );
    assert_eq!(status, 409, "{answer}");
    let (_, _, message) = refusal(&answer);
    assert!(message.contains("carries no refresh token"), "{message}");
    let (status, answer) = gateway.console(
        GRANT,
        Method::POST,
        Some(
            &json!({"subscription_id": "pool-agent", "reason": "story", "harness": "weles",
                     "document": json!({"tokens": {"refresh_token": "sk-ort"}}).to_string()}),
        ),
    );
    assert_eq!(status, 400, "{answer}");
}

/// The desktop's path end to end: the binary beside the harness reads the
/// grant and hands it to a gateway elsewhere with the console's bearer on
/// stdin; the gateway stores it, the provider refuses the made-up token, and
/// a row the ledger had disowned reads active again.
#[test]
fn a_grant_handed_through_a_gateway_gets_the_providers_verdict_and_repairs_the_row() {
    let directory = TestDirectory::new("grant-through-gateway");
    let gateway = Gateway::start("grant-through-gateway", &[(AGENT, "codex", "pool-agent")]);
    let home = home_with_every_harness(&directory, &[PRIMARY]);
    let (status, refreshed) = gateway.console(
        "/v1/admin/subscription-pool/refresh",
        Method::POST,
        Some(&json!({"provider": "codex", "reason": "story"})),
    );
    assert_eq!(status, 200, "{refreshed}");
    let state = |report: &Value| {
        report["subscriptions"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"] == "pool-agent"))
            .map(|row| row["credential"]["state"].clone())
            .unwrap_or(Value::Null)
    };
    let (status, stdout, stderr) = brama(
        gateway.vault(),
        &[
            "subscription",
            "import",
            "codex",
            "--from",
            "codex",
            "--subscription-id",
            "pool-agent",
            "--reason",
            "story",
            "--home",
            home.to_str().unwrap(),
            "--gateway",
            gateway.origin(),
            "--json",
        ],
        Some(CONSOLE_BEARER),
    );
    assert_eq!(
        status, 1,
        "a made-up grant is refused by the provider:\n{stdout}{stderr}"
    );
    let verdict: Value = serde_json::from_str(&stdout).expect("a JSON verdict");
    assert_eq!(verdict["account"], "codex@wisent.ai");
    assert_eq!(verdict["result"], "failed");
    assert!(
        verdict["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("codex, the harness that held it")
                && detail.contains("would not serve it")),
        "the provider, not Brama, refused it: {verdict}"
    );
    let (_, after) = gateway.console(gateway::POOL, Method::GET, None);
    assert_eq!(
        state(&after),
        "active",
        "the handed-over grant is the sign-in the ledger waited for: {after}"
    );
    let (status, _, stderr) = brama(
        gateway.vault(),
        &[
            "subscription",
            "import",
            "codex",
            "--from",
            "codex",
            "--subscription-id",
            "pool-agent",
            "--reason",
            "story",
            "--home",
            home.to_str().unwrap(),
            "--gateway",
            gateway.origin(),
        ],
        Some(""),
    );
    assert_eq!(status, 1);
    assert!(stderr.contains("stdin was empty"), "{stderr}");
}

/// The pool routed every Claude call through one rate-limited account while
/// the operator's machine held two more with quota; adding them needed a
/// sign-in window. A grant handed over for an id the pool does not hold yet
/// creates that member under the named provider, and the pool report lists
/// it beside the others at once. A provider Brama cannot renew grants for
/// is refused before anything is stored.
#[test]
fn a_grant_for_an_id_the_pool_does_not_hold_yet_joins_the_pool() {
    let directory = TestDirectory::new("grant-joins-pool");
    let gateway = Gateway::start("grant-joins-pool", &[(AGENT, "codex", "pool-agent")]);
    let home = home_with_every_harness(&directory, &[PRIMARY]);
    let (status, stdout, stderr) = brama(
        gateway.vault(),
        &[
            "subscription",
            "import",
            "claude-code",
            "--from",
            "claude",
            "--subscription-id",
            "brama-sub-pool-agent-claude-secondary",
            "--reason",
            "story",
            "--home",
            home.to_str().unwrap(),
            "--gateway",
            gateway.origin(),
            "--json",
        ],
        Some(CONSOLE_BEARER),
    );
    assert_eq!(
        status, 1,
        "a made-up grant is refused by the provider, after it is stored:\n{stdout}{stderr}"
    );
    let verdict: Value = serde_json::from_str(&stdout).expect("a JSON verdict");
    assert_eq!(
        verdict["subscription_id"],
        "brama-sub-pool-agent-claude-secondary"
    );
    assert_eq!(verdict["provider"], "claude-code");
    assert!(
        verdict["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("is stored")),
        "the grant was stored before the provider's verdict: {verdict}"
    );
    let (_, pool) = gateway.console(gateway::POOL, Method::GET, None);
    let row = pool["subscriptions"]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["id"] == "brama-sub-pool-agent-claude-secondary")
        })
        .cloned();
    assert!(
        row.is_some(),
        "the new member is in the pool report: {pool}"
    );
    assert_eq!(row.unwrap()["provider"], "claude-code");

    let (status, answer) = gateway.console(
        GRANT,
        Method::POST,
        Some(&json!({"subscription_id": "brama-sub-pool-agent-openrouter-secondary", "reason": "story",
            "provider": "openrouter", "document": "{\"key\":\"x\"}"})),
    );
    assert_eq!(status, 400, "{answer}");
    assert!(
        answer
            .to_string()
            .contains("not a provider whose grants Brama keeps"),
        "{answer}"
    );
    let (status, answer) = gateway.console(
        GRANT,
        Method::POST,
        Some(
            &json!({"subscription_id": "brama-sub-pool-agent-kimi-secondary", "reason": "story",
            "document": "{\"key\":\"x\"}"}),
        ),
    );
    assert_eq!(status, 404, "{answer}");
    assert!(answer.to_string().contains("name its provider"), "{answer}");
}
