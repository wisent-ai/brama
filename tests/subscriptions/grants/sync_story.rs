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

    let (status, again, stderr) = sync(&home, &gateway, CONSOLE_BEARER);
    assert_eq!(
        status, 0,
        "a second sweep imports nothing:\n{again}\n{stderr}"
    );
    assert_eq!(again["ok"], true, "{again}");
    for row in again["grants"].as_array().unwrap() {
        if row["result"] != "unnamed" {
            assert_eq!(row["result"], "present", "{row}");
        }
    }

    let (status, _, stderr) = sync(&home, &gateway, "");
    assert_eq!(status, 1);
    assert!(stderr.contains("stdin was empty"), "{stderr}");
}
