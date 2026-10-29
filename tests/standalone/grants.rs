//! A standalone launch reads the grants file its launcher names before it
//! serves anything, and refuses one it must not trust.
//!
//! The real `brama serve --port 0 --local-credentials-stdin` is started with an
//! empty provider map on standard input and `BRAMA_STANDALONE_GRANTS_FILE`
//! naming a file under this checkout's build directory:
//!
//! - a grants file the owner's group can read is refused naming the file and
//!   "must be readable by its owner only", and the process exits failing;
//! - an owner-only grants file that is not a map from subscription to grant is
//!   refused naming the file, and the process exits failing.
//!
//! Every run's command, exit status and standard error stay in report.json
//! under the checkout's ignored `.build/journeys/standalone-grants/<run>`,
//! never in Cargo's temporary directory.
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

struct Run {
    root: PathBuf,
    report: Value,
}

impl Run {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".build")
            .join("journeys")
            .join("standalone-grants")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("state")).expect("create the run directory");
        Self {
            root,
            report: json!({ "runs": [] }),
        }
    }

    /// A grants file holding `text`, with `mode` as its permission bits.
    fn grants(&self, name: &str, text: &str, mode: u32) -> PathBuf {
        let path = self.root.join(name);
        std::fs::write(&path, text).expect("write the grants file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .expect("set the grants file's mode");
        path
    }

    /// Start the real gateway on `grants`; its exit status and stderr.
    fn serve(&mut self, case: &str, grants: &Path) -> (bool, String) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_brama"))
            .args(["serve", "--port", "0", "--local-credentials-stdin"])
            .env("BRAMA_STANDALONE_GRANTS_FILE", grants)
            .env("BRAMA_STATE_DIR", self.root.join("state"))
            .env("NO_COLOR", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("brama serve starts");
        child
            .stdin
            .take()
            .expect("the gateway's stdin")
            .write_all(b"{}")
            .expect("hand the gateway an empty provider map");
        let output = child.wait_with_output().expect("brama serve ends");
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        self.report["runs"]
            .as_array_mut()
            .expect("runs")
            .push(json!({
                "case": case, "grants": grants, "exit": output.status.code(), "stderr": stderr,
            }));
        self.save();
        (output.status.success(), stderr)
    }

    fn save(&self) {
        std::fs::write(self.root.join("report.json"), self.report.to_string())
            .expect("write report.json");
    }
}

/// The owner's read and write bits, and the same with the group's read bit.
fn modes() -> (u32, u32) {
    let owner = brama::core::mode::OWNER_READ_WRITE;
    (owner, owner | brama::core::mode::GROUP_READ)
}

#[test]
fn a_grants_file_others_can_read_is_refused_before_serving() {
    let mut run = Run::new();
    let (_, shared) = modes();
    let grants = run.grants("shared.json", "{}", shared);
    let (served, stderr) = run.serve("group-readable", &grants);
    assert!(
        !served,
        "a group-readable grants file must stop the start: {stderr}"
    );
    assert!(
        stderr.contains("must be readable by its owner only"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&grants.display().to_string()),
        "the refusal names the file: {stderr}"
    );
}

#[test]
fn a_grants_file_that_is_no_grant_map_is_refused_before_serving() {
    let mut run = Run::new();
    let (owner, _) = modes();
    let grants = run.grants("malformed.json", "[\"not a map\"]", owner);
    let (served, stderr) = run.serve("not-a-map", &grants);
    assert!(
        !served,
        "a grants file that is no map must stop the start: {stderr}"
    );
    assert!(
        stderr.contains("is not a map from subscription to grant"),
        "{stderr}"
    );
}
