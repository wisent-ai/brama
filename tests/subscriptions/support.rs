//! Real CLI execution with retained evidence and gateway identity checked before mutation.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    process::{Command, Output, Stdio},
};

pub(super) struct Run {
    binary: String,
    directory: PathBuf,
    pub(super) report: Value,
    destination: Vec<String>,
    gateway_revision: String,
}

impl Run {
    pub(super) fn new() -> Self {
        let binary = required("BRAMA");
        let gateway_revision = required("GATEWAY_SOURCE_REVISION");
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
            "binary": binary, "binary_sha256": checksum, "commands": [],
            "expected_gateway_source_revision": gateway_revision});
        let mut run = Self {
            binary,
            directory,
            report,
            gateway_revision,
            destination: vec![
                "--gateway-consumer".into(),
                required("CONSUMER"),
                "--bearer-role".into(),
                required("BEARER_ROLE"),
                "--json".into(),
            ],
        };
        run.pool(false);
        run
    }

    pub(super) fn command(&mut self, arguments: &[&str]) -> Output {
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

    pub(super) fn save(&self) {
        std::fs::write(
            self.directory.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .expect("retain real execution evidence");
    }

    fn pool(&mut self, refresh: bool) -> Value {
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
        self.report["gateway_build"] = report["build"].clone();
        self.save();
        let revision = report["build"]["source_revision"]
            .as_str()
            .expect("responding gateway must identify its source revision");
        assert_eq!(
            revision, self.gateway_revision,
            "the answering gateway does not run the selected qualification revision"
        );
        report
    }

    pub(super) fn member(&mut self, id: &str, refresh: bool) -> Value {
        let report = self.pool(refresh);
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

pub(super) fn required(key: &str) -> String {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value,
        Ok(_) => panic!("{key} must not be empty for the real capacity flow"),
        Err(error) => {
            panic!("{key} is required in the test environment for the real capacity flow: {error}")
        }
    }
}
