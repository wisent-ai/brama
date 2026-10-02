//! Real subscription sign-in on the Stado-selected Weles worker.
//! Use an isolated Google subscription; the phone case needs its owner's approval.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{env, fs, panic::AssertUnwindSafe, path::PathBuf, process::{Command, Output}};

struct Evidence {
    path: PathBuf,
    data: Value,
}

impl Evidence {
    fn save(&self) {
        fs::write(&self.path, serde_json::to_vec_pretty(&self.data).unwrap()).unwrap();
    }

    fn run(&mut self, program: &str, args: &[&str]) -> Output {
        let output = Command::new(program).args(args).output().unwrap();
        self.data["commands"].as_array_mut().unwrap().push(json!({
            "program": program, "args": args, "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    fn json(&mut self, program: &str, args: &[&str]) -> Value {
        let output = self.run(program, args);
        assert!(output.status.success(), "{} {:?}: {}", program, args, String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&output.stdout).expect("product must return its JSON result")
    }
}

fn required(name: &str) -> String {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| panic!("{name} must identify the isolated real authentication fixture"))
}

fn subscription<'a>(pool: &'a Value, id: &str) -> &'a Value {
    pool["subscriptions"].as_array().expect("subscription inventory")
        .iter().find(|row| row["id"] == id).expect("the exact fixture subscription must exist")
}

fn journey(evidence: &mut Evidence) {
    let binary = env::var("BRAMA_REAL_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_brama").into());
    let identity = evidence.json(&binary, &["version"]);
    assert_eq!(identity["source_revision"], brama::build_info().source_revision);
    evidence.data["binary_sha256"] = json!(hex::encode(Sha256::digest(fs::read(&binary).unwrap())));
    let provider = required("BRAMA_REAL_PROVIDER");
    let id = required("BRAMA_REAL_SUBSCRIPTION_ID");
    let login = required("BRAMA_REAL_LOGIN_ITEM");
    let account = required("BRAMA_REAL_ACCOUNT");
    let method = required("BRAMA_REAL_SECOND_FACTOR");
    assert!(method == "phone" || method == "authenticator", "the journey must exercise an actual second factor");
    let worker_revision = required("BRAMA_REAL_WELES_REVISION");
    let stado = env::var("BRAMA_STADO_BIN").unwrap_or_else(|_| {
        PathBuf::from(required("HOME")).join(".stado/bin/stado").to_string_lossy().into_owned()
    });
    let base = env::var("BRAMA_WELES_URL").ok().filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            let consumer = env::var("BRAMA_WELES_ADMISSION_CONSUMER").unwrap_or_else(|_| "operator".into());
            evidence.json(&stado, &["service", "directory", "connect", "weles-admission",
                "--consumer", &consumer, "--no-verify", "--json"])["url"]
                .as_str().expect("Stado must name the managed worker").to_owned()
        });
    let url = format!("{}/healthz", base.trim().trim_end_matches('/'));
    let response = reqwest::blocking::get(&url).expect("the real Weles worker must be reachable");
    let status = response.status().as_u16();
    let body = response.text().unwrap();
    evidence.data["worker_health"] = json!({ "url": url, "http_status": status, "body": body });
    evidence.save();
    assert_eq!(status, 200);
    let health: Value = serde_json::from_str(&body).expect("Weles health JSON");
    assert_eq!(health["sourceRevision"], worker_revision);
    evidence.data["account"] = json!(account);
    evidence.data["subscription_id"] = json!(id);
    evidence.data["worker_revision"] = json!(worker_revision);
    evidence.data["second_factor"] = json!(method);

    let before = evidence.json(&binary, &["subscriptions", "--json"]);
    let original = subscription(&before, &id);
    assert_eq!(original["provider"], provider);
    assert_ne!(original["retired"], true, "the fixture must not be retired");
    let refused = evidence.run(&binary, &["subscription", "sign-in", &provider,
        "--subscription-id", &id, "--login-item", &login, "--reason", "", "--json"]);
    assert!(!refused.status.success(), "an empty audit reason must be refused");
    let after_refusal = evidence.json(&binary, &["subscriptions", "--json"]);
    assert_eq!(subscription(&after_refusal, &id)["sign_in"], original["sign_in"]);
    assert_eq!(subscription(&after_refusal, &id)["credential"], original["credential"]);

    let reason = format!("real-authentication-{}", uuid::Uuid::new_v4());
    let started = chrono::Utc::now();
    evidence.data["authentication_started_at"] = json!(started.to_rfc3339());
    evidence.save();
    let signed = evidence.json(&binary, &["subscription", "sign-in", &provider,
        "--subscription-id", &id, "--login-item", &login, "--reason", &reason, "--json"]);
    evidence.data["sign_in"] = signed.clone();
    evidence.save();
    assert_eq!(signed["result"], "signed_in");
    assert_eq!(signed["subscription_id"], id);
    assert_eq!(signed["login_item"], login);
    assert_eq!(signed["account"], account);
    assert_eq!(signed["reason"], reason);
    assert_eq!(signed["source_revision"], worker_revision);
    assert_eq!(signed["second_factor"]["required"], true);
    assert_eq!(signed["second_factor"]["method"], method);
    assert_eq!(signed["refresh"]["result"], "refreshed");
    assert!(signed["run_id"].as_str().is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()));

    // A new CLI process reads the saved journal and the verified credential.
    let after = evidence.json(&binary, &["subscriptions", "--json"]);
    let saved = subscription(&after, &id);
    assert_eq!(saved["sign_in"]["reason"], reason);
    assert_eq!(saved["sign_in"]["result"], "signed_in");
    assert_eq!(saved["sign_in"]["run_id"], signed["run_id"]);
    assert_eq!(saved["credential"]["state"], "active");
    assert_eq!(saved["second_factor"]["required"], true,
        "successful 2FA must not be reported as an account that needs no second factor");
    assert_eq!(saved["second_factor"]["method"], method);

    if method == "phone" {
        let weles = env::var("WELES_REAL_BIN").unwrap_or_else(|_| "weles".into());
        let requests = evidence.json(&weles, &["operator-requests", "list", "--json"]);
        let matching: Vec<_> = requests.as_array().expect("worker request list").iter().filter(|request| {
            request["kind"] == "google-push-approval" && request["account"] == account
                && request["opened_at"].as_str().and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                    .is_some_and(|opened| opened >= started)
        }).collect();
        assert_eq!(matching.len(), 1, "one actual phone request must belong to this fresh sign-in");
        let request = matching[0];
        assert_eq!(request["approved"], true);
        assert!(request["closed_at"].as_str().is_some());
        assert_ne!(request["abandoned"], true);
        evidence.data["phone_request"] = request.clone();
    }
}

#[test]
#[ignore = "requires an isolated real Google subscription, managed Weles and the account owner's phone"]
fn real_subscription_second_factor() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".build/real-tests/authentication").join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&directory).unwrap();
    let mut evidence = Evidence {
        path: directory.join("report.json"),
        data: json!({ "source_revision": brama::build_info().source_revision,
            "started_at": chrono::Utc::now().to_rfc3339(), "status": "running", "commands": [] }),
    };
    evidence.save();
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| journey(&mut evidence)));
    evidence.data["finished_at"] = json!(chrono::Utc::now().to_rfc3339());
    evidence.data["status"] = json!(if outcome.is_ok() { "passed" } else { "failed" });
    if let Err(payload) = &outcome {
        evidence.data["error"] = json!(payload.downcast_ref::<String>().map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied()).unwrap_or("test panicked"));
    }
    evidence.save();
    println!("authentication evidence: {}", evidence.path.display());
    if let Err(error) = outcome { std::panic::resume_unwind(error); }
}
