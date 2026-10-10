//! Real gateway/provider reports; this flow never consumes a reset.
//! Both journeys require BRAMA, CONSUMER and BEARER_ROLE.
//! Reset reporting additionally requires MEMBER and EXPECTED_CREDITS.
//! Discovery requires EXPECTED_ACCOUNT, EXPECTED_HARNESS_PROVIDER and EXPECTED_PROVIDER.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    process::{Command, Output, Stdio},
};

struct Run {
    binary: String,
    directory: PathBuf,
    report: Value,
    destination: Vec<String>,
}

impl Run {
    fn new() -> Self {
        let binary = required("BRAMA");
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/real-tests/subscriptions")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&directory).expect("create retained run directory");
        let mut command = Command::new("git");
        command
            .args(["rev-parse", "HEAD"])
            .current_dir(env!("CARGO_MANIFEST_DIR"));
        let revision = stado_wait::output(&mut command).expect("read source revision");
        assert!(revision.status.success());
        let bytes = std::fs::read(&binary).expect("read tested binary for checksum");
        let checksum = hex::encode(Sha256::digest(bytes));
        let report = json!({"source_revision": String::from_utf8_lossy(&revision.stdout).trim(),
            "binary": binary, "binary_sha256": checksum, "commands": []});
        Self {
            binary,
            directory,
            report,
            destination: vec![
                "--gateway-consumer".into(),
                required("CONSUMER"),
                "--bearer-role".into(),
                required("BEARER_ROLE"),
                "--json".into(),
            ],
        }
    }

    fn command(&mut self, arguments: &[&str]) -> Output {
        let arguments: Vec<_> = arguments
            .iter()
            .map(|arg| (*arg).to_owned())
            .chain(self.destination.iter().cloned())
            .collect();
        let mut command = Command::new(&self.binary);
        command.args(&arguments).stdin(Stdio::null());
        let output = stado_wait::output(&mut command).expect("execute real Brama command");
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "command": self.binary, "arguments": arguments, "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout), "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    fn save(&self) {
        std::fs::write(
            self.directory.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .expect("retain real execution evidence");
    }

    fn member(&mut self, id: &str, refresh: bool) -> Value {
        let output = if refresh {
            self.command(&["subscription", "list", "--refresh-usage"])
        } else {
            self.command(&["subscription", "list"])
        };
        assert!(
            output.status.success(),
            "subscription list refused: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: Value = serde_json::from_slice(&output.stdout).expect("real pool report JSON");
        let mut rows = report["subscriptions"]
            .as_array()
            .expect("pool subscriptions")
            .iter()
            .filter(|row| row["id"] == id);
        let row = rows.next().expect("selected real member exists").clone();
        assert!(rows.next().is_none(), "member identity is not unique");
        row
    }
}

fn required(key: &str) -> String {
    match std::env::var(key) {
        Ok(value) => value,
        Err(error) => {
            panic!("{key} is required in the test environment for the real capacity flow: {error}")
        }
    }
}

#[test]
#[ignore = "Requires a real gateway, independent Brama grant and observed provider credit count"]
fn saved_reset_report_persists_without_consumption() {
    let mut run = Run::new();
    let member = required("MEMBER");
    let expected: u64 = required("EXPECTED_CREDITS")
        .parse()
        .expect("observed provider credit count");
    let before = run.member(&member, false);
    let fresh = run.member(&member, true);
    assert!(fresh["resets"]["error"].is_null(), "{}", fresh["resets"]);
    let offer = &fresh["resets"]["offer"];
    assert_eq!(offer["available_count"].as_u64(), Some(expected));
    let observed = offer["observed_at_ms"]
        .as_i64()
        .expect("provider report observation");
    let attempted = fresh["resets"]["attempted_at_ms"]
        .as_i64()
        .expect("recorded read attempt");
    assert!(observed <= attempted);
    let persisted = run.member(&member, false);
    assert_eq!(&persisted["resets"]["offer"], offer);
    assert_eq!(persisted["reset_redemption"], before["reset_redemption"]);
    let provider = fresh["provider"].as_str().expect("member provider");
    let missing = run.command(&["subscription", "reset", provider, "--member", &member]);
    assert!(
        !missing.status.success(),
        "reset without a reason was accepted"
    );
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--reason"));
    let mismatch = run.command(&[
        "subscription",
        "reset",
        "nonexistent-provider",
        "--member",
        &member,
        "--reason",
        "real test: provider mismatch must not consume saved capacity",
    ]);
    assert!(!mismatch.status.success(), "provider mismatch was accepted");
    let refusal = format!(
        "{}{}",
        String::from_utf8_lossy(&mismatch.stdout),
        String::from_utf8_lossy(&mismatch.stderr)
    );
    assert!(
        refusal.contains("belongs to") && refusal.contains(provider),
        "provider mismatch did not expose the member's actual provider: {refusal}"
    );
    let after = run.member(&member, false);
    assert_eq!(after["reset_redemption"], before["reset_redemption"]);
    run.report["result"] = json!("passed");
    run.save();
}

#[test]
#[ignore = "Requires real harness accounts, gateway discovery dependencies and independent sign-in"]
fn discovered_harness_account_persists_once_with_its_observed_plan() {
    let mut run = Run::new();
    let account = required("EXPECTED_ACCOUNT");
    let harness_provider = required("EXPECTED_HARNESS_PROVIDER");
    let provider = required("EXPECTED_PROVIDER");
    let mut command = Command::new("omp");
    command.args(["usage", "--json"]).stdin(Stdio::null());
    let observed = stado_wait::output(&mut command).expect("read real harness usage");
    run.report["harness_observation"] = json!({
        "command": ["omp", "usage", "--json"],
        "exit_status": observed.status.code(),
        "stdout": String::from_utf8_lossy(&observed.stdout),
        "stderr": String::from_utf8_lossy(&observed.stderr),
    });
    run.save();
    assert!(
        observed.status.success(),
        "harness usage refused: {}",
        String::from_utf8_lossy(&observed.stderr)
    );
    let document: Value =
        serde_json::from_slice(&observed.stdout).expect("real harness usage JSON");
    let reports = document["reports"].as_array().expect("harness reports");
    let observed_account = reports
        .iter()
        .find(|row| {
            row["provider"] == harness_provider
                && row["metadata"]["email"]
                    .as_str()
                    .is_some_and(|email| email.eq_ignore_ascii_case(&account))
        })
        .expect("the selected account must actually be present in the harness");
    let plan = observed_account["metadata"]["planType"]
        .as_str()
        .expect("the selected real report must identify its plan");
    let discovery = run.command(&["subscription", "discover"]);
    assert!(
        discovery.status.success(),
        "discovery refused: {} {}",
        String::from_utf8_lossy(&discovery.stdout),
        String::from_utf8_lossy(&discovery.stderr)
    );
    let discovered: Value =
        serde_json::from_slice(&discovery.stdout).expect("discovery report JSON");
    assert_eq!(discovered["ok"], true, "{discovered}");
    let rows = discovered["accounts"]
        .as_array()
        .expect("discovered accounts");
    let row = rows
        .iter()
        .find(|row| {
            row["provider"] == provider
                && row["account"]
                    .as_str()
                    .is_some_and(|email| email.eq_ignore_ascii_case(&account))
        })
        .expect("selected observed account is enrolled");
    assert_eq!(row["registered"], true, "{row}");
    let id = row["id"].as_str().expect("registered member id");
    let persisted = run.member(id, false);
    assert_eq!(persisted["provider"], provider);
    assert_eq!(persisted["discovery"]["account"], account.to_lowercase());
    assert_eq!(persisted["discovery"]["plan"], plan);
    assert!(persisted["discovery"]["sources"]
        .as_array()
        .expect("retained discovery sources")
        .iter()
        .any(|source| source == "omp usage --json"));
    let discovered_at = persisted["discovery"]["discovered_at_ms"]
        .as_i64()
        .expect("first discovery time");
    let repeated = run.command(&["subscription", "discover"]);
    assert!(
        repeated.status.success(),
        "repeated discovery refused: {} {}",
        String::from_utf8_lossy(&repeated.stdout),
        String::from_utf8_lossy(&repeated.stderr)
    );
    let after = run.member(id, false);
    assert_eq!(after["discovery"]["discovered_at_ms"], discovered_at);
    assert_eq!(
        after["discovery"]["account"],
        persisted["discovery"]["account"]
    );
    let listed = run.command(&["subscription", "list"]);
    assert!(listed.status.success());
    let pool: Value = serde_json::from_slice(&listed.stdout).expect("pool JSON");
    let mut matches = pool["subscriptions"]
        .as_array()
        .expect("pool subscriptions")
        .iter()
        .filter(|member| {
            member["provider"] == provider
                && member["retired"] != true
                && member["account"]
                    .as_str()
                    .is_some_and(|email| email.eq_ignore_ascii_case(&account))
        });
    assert_eq!(
        matches.next().expect("persisted discovered member")["id"],
        id
    );
    assert!(matches.next().is_none(), "discovery duplicated the account");
    run.report["result"] = json!("passed");
    run.save();
}
