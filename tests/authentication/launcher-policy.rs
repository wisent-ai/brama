//! Real launcher policy CLI against the operator's control document and
//! isolated refusal inputs. Neither the service nor the vault is changed.

use serde_json::{json, Value};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Evidence {
    directory: PathBuf,
    report: Value,
}

impl Evidence {
    fn invoke(&mut self, program: &str, arguments: &[&str]) -> Output {
        let mut command = Command::new(program);
        command.args(arguments).stdin(Stdio::null());
        let output = stado_wait::output(&mut command).expect("execute real product command");
        self.report["commands"]
            .as_array_mut()
            .expect("command log")
            .push(json!({
                "program": program, "arguments": arguments, "exit_status": output.status.code(),
                "stdout": String::from_utf8_lossy(&output.stdout),
                "stderr": String::from_utf8_lossy(&output.stderr),
            }));
        self.save();
        output
    }

    fn save(&self) {
        std::fs::write(
            self.directory.join("report.json"),
            serde_json::to_vec_pretty(&self.report).expect("serialize evidence"),
        )
        .expect("retain launcher policy evidence");
    }
}

impl Drop for Evidence {
    fn drop(&mut self) {
        self.report["passed"] = json!(!std::thread::panicking());
        self.save();
    }
}

fn policy_run(evidence: &mut Evidence, binary: &str, config: &Path, name: &str) -> Output {
    let output = evidence.directory.join(name);
    std::fs::create_dir(&output).expect("isolate launcher policy outputs");
    let allowed = output.join("allowed");
    let aliases = output.join("aliases");
    let backend = output.join("backend");
    let lifetime = output.join("lifetime");
    evidence.invoke(
        binary,
        &[
            "launcher",
            "policy",
            "--config",
            config.to_str().expect("UTF-8 control path"),
            "--allowed",
            allowed.to_str().expect("UTF-8 allowed path"),
            "--aliases",
            aliases.to_str().expect("UTF-8 aliases path"),
            "--backend",
            backend.to_str().expect("UTF-8 backend path"),
            "--lifetime",
            lifetime.to_str().expect("UTF-8 lifetime path"),
        ],
    )
}

#[test]
#[ignore = "Requires the actual Brama binary and operator-owned control document"]
fn managed_capability_lifetime_is_exported_or_refused_by_real_cli() {
    let binary = std::env::var("BRAMA").expect("BRAMA must name the tested binary");
    let control = PathBuf::from(
        std::env::var("BRAMA_CONTROL_CONFIG")
            .expect("BRAMA_CONTROL_CONFIG must name the actual service policy"),
    );
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/real-tests/launcher-policy")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&directory).expect("create isolated evidence directory");
    let mut evidence = Evidence {
        directory,
        report: json!({
            "binary": binary, "control": control, "commands": [], "passed": false,
        }),
    };
    let root = env!("CARGO_MANIFEST_DIR");
    let revision = evidence.invoke("git", &["-C", root, "rev-parse", "HEAD"]);
    assert!(revision.status.success(), "cannot identify source revision");
    let revision = String::from_utf8(revision.stdout).expect("revision UTF-8");
    let revision = revision.trim();
    evidence.report["source_revision"] = json!(revision);
    let status = evidence.invoke(
        "git",
        &["-C", root, "status", "--porcelain", "--untracked-files=all"],
    );
    assert!(
        status.status.success() && status.stdout.is_empty(),
        "qualification requires a clean committed source tree"
    );
    let version = evidence.invoke(&binary, &["version"]);
    assert!(version.status.success(), "Brama build identity refused");
    let version: Value = serde_json::from_slice(&version.stdout).expect("Brama version JSON");
    assert_eq!(
        version["source_revision"].as_str(),
        Some(revision),
        "binary is not built from this revision"
    );
    evidence.report["build"] = version;
    evidence.save();

    let document: Value =
        serde_json::from_slice(&std::fs::read(&control).expect("read actual control document"))
            .expect("parse actual control document");
    let lifetime = document
        .pointer("/services/brama/capability_lifetime_seconds")
        .and_then(Value::as_u64)
        .and_then(NonZeroU64::new)
        .expect("actual policy must declare a positive capability lifetime");
    let accepted = policy_run(&mut evidence, &binary, &control, "accepted");
    assert!(
        accepted.status.success(),
        "actual control policy refused: {}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let exported = std::fs::read_to_string(evidence.directory.join("accepted/lifetime"))
        .expect("read persisted launcher lifetime");
    assert_eq!(
        exported,
        lifetime.to_string(),
        "gateway would receive a different lifetime"
    );
    evidence.report["persisted_lifetime_seconds"] = json!(lifetime.get());
    evidence.save();

    for (name, value) in [("missing", None), ("not-a-number", Some(json!("invalid")))] {
        let mut rejected = document.clone();
        let policy = rejected
            .pointer_mut("/services/brama")
            .and_then(Value::as_object_mut)
            .expect("actual service policy object");
        match value {
            Some(value) => {
                policy.insert("capability_lifetime_seconds".into(), value);
            }
            None => {
                policy.remove("capability_lifetime_seconds");
            }
        }
        let fixture = evidence.directory.join(format!("{name}.json"));
        std::fs::write(
            &fixture,
            serde_json::to_vec(&rejected).expect("serialize policy"),
        )
        .expect("write isolated refusal input");
        let refused = policy_run(&mut evidence, &binary, &fixture, name);
        assert!(!refused.status.success(), "{name} lifetime was admitted");
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("capability_lifetime_seconds"),
            "{name} refusal did not name the failed field: {}",
            String::from_utf8_lossy(&refused.stderr)
        );
        assert!(
            !evidence.directory.join(name).join("lifetime").exists(),
            "{name} refusal persisted an invalid lifetime"
        );
    }
}
