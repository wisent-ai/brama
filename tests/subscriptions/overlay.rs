//! Actual overlay persistence API against isolated files; no vault or provider access.
use brama::gateway::broker::{donated_add, donated_remove};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Command};

struct Evidence {
    directory: PathBuf,
    report: Value,
    previous: Option<std::ffi::OsString>,
}

impl Drop for Evidence {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var("BRAMA_DONATED_SUBSCRIPTIONS_FILE", value),
            None => std::env::remove_var("BRAMA_DONATED_SUBSCRIPTIONS_FILE"),
        }
        self.report["passed"] = json!(!std::thread::panicking());
        std::fs::write(self.directory.join("report.json"), self.report.to_string())
            .expect("retain overlay journey result");
    }
}

#[test]
#[ignore = "Runs the actual overlay persistence API on isolated files with a revision-bound test binary"]
fn overlay_refuses_unreadable_state_without_replacing_it() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target/real-tests/subscription-overlay")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&directory).expect("create evidence directory");
    let mut command = Command::new("git");
    command
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    let revision = stado_wait::output(&mut command).expect("read source revision");
    assert!(revision.status.success());
    let source = String::from_utf8(revision.stdout).expect("source revision");
    let mut evidence = Evidence {
        directory: directory.clone(),
        previous: std::env::var_os("BRAMA_DONATED_SUBSCRIPTIONS_FILE"),
        report: json!({"source_revision": source.trim(), "build": brama::build_info::current(),
            "command": std::env::args().collect::<Vec<_>>(), "failures": []}),
    };
    assert_eq!(
        brama::build_info::SOURCE_REVISION,
        source.trim(),
        "test binary is not the recorded source revision"
    );
    let path = directory.join("subscriptions.json");
    std::env::set_var("BRAMA_DONATED_SUBSCRIPTIONS_FILE", &path);
    for invalid in ["{", "{}", "{\"items\":null}"] {
        std::fs::write(&path, invalid).expect("write isolated invalid overlay");
        let failure = donated_remove("fixture-member").expect_err("invalid overlay was replaced");
        evidence.report["failures"]
            .as_array_mut()
            .unwrap()
            .push(json!({"input": invalid, "error": failure}));
        assert!(
            failure.contains(path.to_str().unwrap()),
            "failed file is not identified"
        );
        assert!(
            failure.contains("line") && failure.contains("column"),
            "JSON decoder cause was discarded: {failure}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            invalid,
            "refusal changed the overlay"
        );
    }
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let observed = std::fs::read_to_string(&path).expect_err("fixture must refuse a file read");
    let failure = donated_remove("fixture-member").expect_err("directory accepted as overlay");
    evidence.report["failures"]
        .as_array_mut()
        .unwrap()
        .push(json!({"operation": "read directory as overlay", "error": failure}));
    assert!(failure.contains(path.to_str().unwrap()));
    assert!(
        failure.contains(&observed.to_string()),
        "operating-system cause was discarded"
    );
    assert!(path.is_dir(), "refusal replaced the directory");
    std::fs::remove_dir(&path).unwrap();
    donated_add("fixture-owner", "fixture-member", "codex", None, None)
        .expect("add real overlay member");
    let added: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        added["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["fixture-member"]
    );
    donated_remove("fixture-member").expect("remove real overlay member");
    let removed: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(removed["items"].as_array().unwrap().is_empty());
    evidence.report["added"] = added;
    evidence.report["removed"] = removed;
}
