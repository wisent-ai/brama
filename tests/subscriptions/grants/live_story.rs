//! A real ChatGPT grant, borrowed read-only from OMP, against an isolated
//! gateway and vault. Gateway disables both sweeps: this proof must never
//! rotate the operator's grant or write to the harness that holds it.
use super::*;

#[test]
fn a_real_held_codex_grant_is_accepted_and_probed() {
    let account = std::env::var("BRAMA_REAL_CODEX_ACCOUNT")
        .expect("BRAMA_REAL_CODEX_ACCOUNT must name the existing OMP account to prove");
    let home = std::env::var("BRAMA_REAL_HARNESS_HOME")
        .expect("BRAMA_REAL_HARNESS_HOME must name the home whose OMP store may be read");
    let identities = json!([
        {"client_id": "brama-desktop", "token": CONSOLE_BEARER},
        {"client_id": "brama-pool-agent-client", "token": gateway::AGENT_BEARER,
         "agent_id": AGENT, "allowed_models": ["codex/gpt-6-astra"]},
    ]);
    let gateway = Gateway::start_with(
        "real-held-codex",
        &[(AGENT, "codex", "held-codex")],
        &[(
            "BRAMA_MODEL_ROUTER_CLIENT_IDENTITIES",
            identities.to_string(),
        )],
    );
    let arguments = [
        "subscription",
        "import",
        "codex",
        "--from",
        "omp",
        "--account",
        &account,
        "--subscription-id",
        "held-codex",
        "--reason",
        "prove the existing ChatGPT grant without rotating it",
        "--home",
        &home,
        "--gateway",
        gateway.origin(),
        "--json",
    ];
    let (status, stdout, stderr) = brama(gateway.vault(), &arguments, Some(CONSOLE_BEARER));
    let (pool_status, pool) = gateway.console(gateway::POOL, Method::GET, None);
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
        .join(support::fixture_name("import"));
    std::fs::create_dir_all(&evidence).expect("retain real import evidence");
    let receipt = json!({
        "source_revision": String::from_utf8_lossy(&revision.stdout).trim(),
        "arguments": arguments, "exit_status": status, "stdout": stdout, "stderr": stderr,
        "pool_status": pool_status, "pool": pool,
    });
    std::fs::write(
        evidence.join("import.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    println!("Real held-grant import evidence: {}", evidence.display());
    assert_eq!(
        status,
        0,
        "{stdout}{stderr}; evidence: {}",
        evidence.display()
    );
    let verdict: Value = serde_json::from_str(&stdout).expect("the import verdict");
    assert_eq!(verdict["result"], "signed_in", "{verdict}");
    assert_eq!(verdict["account"], account);
    assert_eq!(pool_status, 200, "{pool}");
    let row = pool["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "held-codex")
        .expect("the stored subscription");
    assert_eq!(row["credential"]["state"], "active", "{row}");
    assert_eq!(
        row["probe"]["ok"], true,
        "the real provider must answer the import probe: {row}"
    );

    let missing = format!("{}@example.invalid", support::fixture_name("absent"));
    let mut refused_arguments = arguments;
    refused_arguments[6] = &missing;
    let (refused_status, refused_stdout, refused_stderr) =
        brama(gateway.vault(), &refused_arguments, Some(CONSOLE_BEARER));
    let (after_status, after) = gateway.console(gateway::POOL, Method::GET, None);
    let refusal = json!({"arguments": refused_arguments, "exit_status": refused_status,
        "stdout": refused_stdout, "stderr": refused_stderr, "pool_status": after_status, "pool": after});
    std::fs::write(
        evidence.join("unheld-account.json"),
        serde_json::to_vec_pretty(&refusal).unwrap(),
    )
    .unwrap();
    assert_ne!(
        refused_status, 0,
        "an account no harness holds must be refused: {refusal}"
    );
    assert_eq!(after_status, 200, "{after}");
    let unchanged = after["subscriptions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "held-codex")
        .unwrap();
    assert_eq!(
        unchanged["probe"], row["probe"],
        "a refused selection must not probe or replace the stored grant"
    );
    assert_eq!(unchanged["credential"], row["credential"]);
    prove_shared_cold_discovery(&gateway, &evidence);
}

fn prove_shared_cold_discovery(gateway: &Gateway, evidence: &Path) {
    // Six signed clients reproduce the deployment's simultaneous first reads.
    const CALLERS: usize = 6;
    let credential_reads = |log: &str| {
        log.lines()
            .filter(|line| line.contains("subscription_models_discovered"))
            .map(|line| {
                line.split_whitespace()
                    .find_map(|field| field.strip_prefix("read="))
                    .expect("discovery must report its credential read count")
                    .parse::<usize>()
                    .expect("the reported read count must be an integer")
            })
            .sum::<usize>()
    };
    let before = credential_reads(&gateway.log());
    let barrier = std::sync::Barrier::new(CALLERS);
    let replies = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..CALLERS)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    gateway.agent("/v1/models", Method::GET, None)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>()
    });
    // The fixture drains stderr on another thread. Wait for the completed
    // discovery events, not an arbitrary delay before counting their reads.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let log = loop {
        let log = gateway.log();
        if log
            .lines()
            .filter(|line| line.contains("subscription_models_discovered"))
            .count()
            >= CALLERS
            || std::time::Instant::now() >= deadline
        {
            break log;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    std::fs::write(
        evidence.join("concurrent-discovery.json"),
        serde_json::to_vec_pretty(&replies).unwrap(),
    )
    .unwrap();
    std::fs::write(evidence.join("gateway.log"), &log).unwrap();
    for (status, body) in replies {
        assert_eq!(status, 200, "{body}");
        assert!(
            body["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| { model["id"] == "codex/gpt-6-astra" && model["available"] == true }),
            "every authenticated client must see the usable subscription: {body}"
        );
    }
    assert_eq!(
        credential_reads(&log) - before,
        1,
        "concurrent catalogue reads must share their cold discovery: {log}"
    );
}
