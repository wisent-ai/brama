//! Every grant a harness holds joins the pool without being named: the
//! sweep the operator asked for when the pool refused Claude while Claude
//! was answering the operator on the same machine.

use super::*;

fn sync(home: &Path, gateway: &Gateway, stdin: &str) -> (i32, Value, String) {
    let (status, stdout, stderr) = brama(
        gateway.vault(),
        &[
            "subscription",
            "sync",
            "--reason",
            "story",
            "--home",
            home.to_str().unwrap(),
            "--gateway",
            gateway.origin(),
            "--json",
        ],
        Some(stdin),
    );
    let report = serde_json::from_str(&stdout).unwrap_or(Value::Null);
    (status, report, stderr)
}

fn slug(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// `omp` refreshed this account: its store now holds new tokens, the way
/// omp's own refresh rewrites the row.
fn rotate_omp_claude_grant(home: &Path, email: &str) {
    let store = Connection::open(home.join(".omp/agent/agent.db")).expect("open omp's store");
    let rotated = json!({
        "access": format!("sk-oat-rotated-{email}"),
        "refresh": format!("sk-ort-rotated-{email}"),
        "expires": 1_789_400_000_000_i64,
        "accountId": format!("acct-{email}"),
        "email": email,
        "orgName": "Wisent",
    })
    .to_string();
    store
        .execute(
            "UPDATE auth_credentials SET data = ?1 WHERE identity_key = ?2",
            rusqlite::params![rotated, format!("email:{email}")],
        )
        .expect("rotate omp's row");
}

/// One sweep imports every held account the pool does not hold; the pool
/// report then lists one member per account under a stable id; a second
/// sweep finds every one present and imports nothing, so the grant Brama
/// now refreshes itself is never overwritten by the harness's stale copy.
#[test]
fn every_held_account_joins_the_pool_once() {
    let directory = TestDirectory::new("sync-joins-pool");
    let gateway = Gateway::start("sync-joins-pool", &[(AGENT, "codex", "pool-agent")]);
    let home = home_with_every_harness(&directory, &[PRIMARY, SECOND]);

    let (status, report, stderr) = sync(&home, &gateway, CONSOLE_BEARER);
    // Made-up grants: every one is stored, the provider refuses the proof,
    // and the sweep says so per grant and exits unsuccessfully.
    assert_eq!(status, 1, "{report}\n{stderr}");
    assert_eq!(report["ok"], false, "{report}");
    let rows = report["grants"].as_array().cloned().unwrap_or_default();
    let named: Vec<&Value> = rows
        .iter()
        .filter(|row| row["result"] != "unnamed")
        .collect();
    assert!(
        !named.is_empty(),
        "the harnesses hold named grants: {report}"
    );
    for row in &named {
        assert_eq!(
            row["result"], "failed",
            "stored, then refused by the provider: {row}"
        );
        assert!(
            row["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("would not serve it")),
            "the provider, not Brama, refused it: {row}"
        );
        let account = row["account"].as_str().expect("a named grant");
        let expected = format!(
            "brama-sub-held-{}-{}",
            slug(row["provider"].as_str().unwrap()),
            slug(account)
        );
        assert_eq!(
            row["subscription_id"], expected,
            "one stable id per account: {row}"
        );
    }
    assert!(
        rows.iter()
            .any(|row| row["result"] == "unnamed" && row["provider"] == "kimi"),
        "a grant whose harness records no account cannot stand for a member: {report}"
    );
    let omp_claude = named
        .iter()
        .filter(|row| row["harness"] == "omp" && row["provider"] == "claude-code")
        .count();
    assert_eq!(
        omp_claude, 2,
        "both enabled omp Claude accounts joined; the lapsed one did not: {report}"
    );

    let (_, pool) = gateway.console(gateway::POOL, Method::GET, None);
    let ids: Vec<&str> = pool["subscriptions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["id"].as_str())
        .collect();
    for row in &named {
        let id = row["subscription_id"].as_str().unwrap();
        assert!(ids.contains(&id), "{id} is in the pool report: {pool}");
    }

    // Every member is borrowed: the second sweep hands each grant over
    // again, and the gateway answers `unchanged` for every one the harness
    // has not rotated, storing and proving nothing. The row says which
    // harness the grant is borrowed from.
    let (status, again, stderr) = sync(&home, &gateway, CONSOLE_BEARER);
    assert_eq!(
        status, 0,
        "a second sweep stores nothing:\n{again}\n{stderr}"
    );
    assert_eq!(again["ok"], true, "{again}");
    for row in again["grants"].as_array().unwrap() {
        if row["result"] != "unnamed" {
            assert_eq!(row["result"], "present", "{row}");
            assert!(
                row["detail"]
                    .as_str()
                    .is_some_and(|detail| detail.contains("already holds")),
                "{row}"
            );
        }
    }
    let (_, pool) = gateway.console(gateway::POOL, Method::GET, None);
    let claude_members: Vec<&Value> = pool["subscriptions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| {
            row["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("brama-sub-held-claude-code-"))
        })
        .collect();
    assert!(!claude_members.is_empty(), "{pool}");
    for member in &claude_members {
        assert_eq!(member["credential"]["borrowed_from"], "omp", "{member}");
    }

    // A borrowed grant is the harness's to rotate. A refresh of the provider
    // leaves it as it stands - nothing is rotated out from under the
    // harness - and the member stays what it was.
    let (status, refreshed) = gateway.console(
        "/v1/admin/subscription-pool/refresh",
        Method::POST,
        Some(&json!({"provider": "claude-code", "reason": "story"})),
    );
    assert_eq!(status, 200, "{refreshed}");
    let (_, pool) = gateway.console(gateway::POOL, Method::GET, None);
    for member in pool["subscriptions"].as_array().unwrap() {
        if member["credential"]["borrowed_from"] == "omp" {
            assert_ne!(
                member["credential"]["state"], "needs_reauthorization",
                "a provider refresh does not touch a borrowed grant: {member}"
            );
        }
    }

    // When the harness rotates its grant, the next sweep stores the new one
    // in the member's place and proves it, instead of leaving the pool on
    // the copy the provider will refuse.
    rotate_omp_claude_grant(&home, PRIMARY);
    let (_, third, _) = sync(&home, &gateway, CONSOLE_BEARER);
    let rotated = third["grants"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["account"] == PRIMARY && row["provider"] == "claude-code")
        .cloned()
        .expect("the rotated account is swept");
    assert_ne!(
        rotated["result"], "present",
        "the rotated grant is stored again: {rotated}"
    );
    assert!(
        rotated["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("is stored")),
        "{rotated}"
    );

    // The sweep resolves where it is writing before it reads a single
    // grant, so an empty stdin is refused by the destination rather than by
    // the handover: one sentence, naming both ways to give it the bearer.
    let (status, _, stderr) = sync(&home, &gateway, "");
    assert_eq!(status, 1);
    assert!(
        stderr
            .contains("a gateway needs the console's bearer: --bearer-item, or the token on stdin"),
        "{stderr}"
    );
}
