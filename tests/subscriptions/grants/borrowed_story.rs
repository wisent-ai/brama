//! The operator's own live Claude account, against an isolated gateway:
//! a grant borrowed from omp stays omp's to refresh.
//!
//! This is the failure the operator reported on 2026-09-20 — `No API key
//! for provider: anthropic` in omp every few hours, each time within the
//! hour of Brama refreshing the same grant: the provider rotates the
//! refresh token, so the copy omp still held was revoked and omp's store
//! recorded `invalid_grant -- Refresh token not found or invalid`.
//!
//! So this story reads omp's own refresh token before and after handing
//! the same grant to a gateway, and the regression it defends against is
//! that token changing. It also reads what the operator reads: the pool of
//! the gateway that serves, which now says who refreshes each member.

use super::*;

/// omp's stored refresh token for one account, read the way omp writes it:
/// one row per identity in its own SQLite store, the grant in `data`.
fn omp_refresh_token(home: &Path, account: &str) -> String {
    let store = Connection::open_with_flags(
        home.join(".omp/agent/agent.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open omp's store read-only");
    let document: String = store
        .query_row(
            "SELECT data FROM auth_credentials
             WHERE provider = 'anthropic' AND identity_key LIKE ?1 AND disabled_cause IS NULL
             ORDER BY updated_at DESC LIMIT 1",
            rusqlite::params![format!("email:{account}%")],
            |row| row.get(0),
        )
        .expect("omp holds an enabled grant for this account");
    let grant: Value = serde_json::from_str(&document).expect("omp's grant is JSON");
    grant["refresh"]
        .as_str()
        .expect("omp's grant carries a refresh token")
        .to_owned()
}

#[test]
fn a_real_held_claude_grant_stays_omps_to_refresh() {
    let account = std::env::var("BRAMA_REAL_CLAUDE_ACCOUNT")
        .expect("BRAMA_REAL_CLAUDE_ACCOUNT must name the live omp Claude account to prove");
    let home = std::env::var("BRAMA_REAL_HARNESS_HOME")
        .expect("BRAMA_REAL_HARNESS_HOME must name the home whose omp store may be read");
    let home = Path::new(&home).to_owned();
    let before = omp_refresh_token(&home, &account);

    // The member the sweep derives for this account: the service on this
    // machine runs `subscription sync`, so the story runs what it runs.
    let member_id = format!(
        "brama-sub-held-claude-code-{}",
        account
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            })
            .collect::<String>()
    );
    let identities = json!([
        {"client_id": "brama-desktop", "token": CONSOLE_BEARER},
        {"client_id": "brama-pool-agent-client", "token": gateway::AGENT_BEARER,
         "agent_id": AGENT, "allowed_models": ["best"]},
    ]);
    let gateway = Gateway::start_with(
        "real-held-claude",
        &[(AGENT, "claude-code", &member_id)],
        &[(
            "BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES",
            identities.to_string(),
        )],
    );
    let arguments = [
        "subscription",
        "sync",
        "--reason",
        "prove the operator's Claude grant is not rotated by Brama",
        "--home",
        home.to_str().expect("a readable home path"),
        "--gateway",
        gateway.origin(),
        "--json",
    ];
    let (status, stdout, stderr) = brama(gateway.vault(), &arguments, Some(CONSOLE_BEARER));
    let revision = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("source revision");
    assert!(
        revision.status.success(),
        "cannot bind the run to its source"
    );
    let evidence = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/real-import-evidence")
        .join(support::fixture_name("borrowed"));
    std::fs::create_dir_all(&evidence).expect("retain the evidence of this run");
    let swept: Value = serde_json::from_str(&stdout).expect("the sweep's report");
    let verdict = swept["grants"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["account"] == account && row["provider"] == "claude-code")
        .cloned()
        .unwrap_or_else(|| panic!("the sweep names this account: {stdout}{stderr} ({status})"));
    assert_eq!(verdict["result"], "imported", "{verdict}");
    // The verdict says who refreshes it from here. Until 2026-09-20 it said
    // "Brama will refresh it from now on" about a grant `renewal` refuses to
    // rotate, which is the sentence an operator would have acted on.
    let detail = verdict["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("omp refreshes it from now on and Brama does not rotate it"),
        "the verdict names the harness as the refresher: {verdict}"
    );

    let (pool_status, pool) = gateway.console(gateway::POOL, Method::GET, None);
    assert_eq!(pool_status, 200, "{pool}");
    let member = pool["subscriptions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["id"] == member_id)
        .cloned()
        .expect("the imported member");
    assert_eq!(member["credential"]["borrowed_from"], "omp", "{member}");
    assert_eq!(member["credential"]["state"], "active", "{member}");

    // An operator's refresh is the forced form of the sweep, and forcing is
    // the one thing that used to rotate this grant at the provider. It must
    // refuse instead, in the harness's name, and leave the token alone.
    let (refresh_status, refreshed) = gateway.console(
        "/v1/admin/subscription-pool/refresh",
        Method::POST,
        Some(&json!({"provider": "claude-code", "reason": "story"})),
    );
    assert_eq!(refresh_status, 200, "{refreshed}");
    assert!(
        refreshed["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("borrowed from omp")),
        "the refresh refuses the borrowed grant by name: {refreshed}"
    );
    let (_, after) = gateway.console(gateway::POOL, Method::GET, None);
    let unrotated = after["subscriptions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["id"] == member_id)
        .cloned()
        .expect("the member survives the refresh");
    assert_eq!(unrotated["credential"]["borrowed_from"], "omp", "{after}");
    assert!(
        unrotated["credential"]["cause"]
            .as_str()
            .is_none_or(|cause| cause.contains("borrowed from omp")),
        "a disowned borrowed grant says the harness holds it: {after}"
    );

    // The operator's own harness still holds the grant it held: nothing
    // rotated the token out from under the session they are signed into.
    let held_after = omp_refresh_token(&home, &account);
    assert_eq!(
        held_after, before,
        "Brama revoked the refresh token omp holds"
    );

    // A refused rotation is not an outage: the sweep brings the grant the
    // harness holds right now, and the member serves again without anybody
    // signing in.
    let (sweep_status, sweep_out, sweep_err) = brama(
        gateway.vault(),
        &[
            "subscription",
            "sync",
            "--reason",
            "story",
            "--home",
            home.to_str().expect("a readable home path"),
            "--gateway",
            gateway.origin(),
            "--json",
        ],
        Some(CONSOLE_BEARER),
    );
    let repaired: Value = serde_json::from_str(&sweep_out).unwrap_or(Value::Null);
    let claude = repaired["grants"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|row| row["account"] == account && row["provider"] == "claude-code")
        .cloned()
        .unwrap_or(Value::Null);
    assert_eq!(
        claude["result"], "imported",
        "the sweep takes the harness's current grant again: {sweep_out}{sweep_err} ({sweep_status})"
    );

    // And the operator can read that answer where the grants are, against
    // the gateway that actually serves.
    let (read_status, read_out, read_err) = brama(
        gateway.vault(),
        &["subscriptions", "--gateway", gateway.origin()],
        Some(CONSOLE_BEARER),
    );
    assert!(
        read_out.contains(&member_id)
            && read_out
                .contains("refreshed by: omp, the harness that holds it; Brama does not rotate it"),
        "the pool names the refresher: {read_out}{read_err}"
    );
    let receipt = json!({
        "source_revision": String::from_utf8_lossy(&revision.stdout).trim(),
        "arguments": arguments,
        "exit_status": status,
        "verdict": verdict,
        "pool": after,
        "pool_read_exit_status": read_status,
        "pool_read": read_out,
        "omp_refresh_token_unchanged": held_after == before,
    });
    std::fs::write(
        evidence.join("borrowed.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    println!("Borrowed-grant evidence: {}", evidence.display());
}

/// Reading another gateway's pool needs the console's bearer and cannot be
/// answered with usage this process did not fetch; both refusals say so.
#[test]
fn reading_another_gateways_pool_refuses_without_a_bearer_and_never_fakes_usage() {
    let gateway = Gateway::start("pool-read-refusals", &[(AGENT, "codex", "pool-agent")]);
    let (status, _, stderr) = brama(
        gateway.vault(),
        &["subscriptions", "--gateway", gateway.origin()],
        Some(""),
    );
    assert_eq!(status, 1);
    assert!(stderr.contains("needs the console's bearer"), "{stderr}");
    let (status, _, stderr) = brama(
        gateway.vault(),
        &[
            "subscriptions",
            "--gateway",
            gateway.origin(),
            "--refresh-usage",
        ],
        Some(CONSOLE_BEARER),
    );
    assert_eq!(status, 1);
    assert!(
        stderr.contains("run it there"),
        "usage is the serving gateway's to fetch: {stderr}"
    );
    let (status, _, stderr) = brama(
        gateway.vault(),
        &["subscriptions", "--bearer-item", "some-item#token"],
        None,
    );
    assert_eq!(status, 1);
    assert!(
        stderr.contains("--bearer-item is for a gateway"),
        "{stderr}"
    );
}
