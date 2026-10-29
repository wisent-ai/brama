//! Real `brama launcher verbs` against the pinned Skarbiec router and a
//! launcher family written for this run, so every construct of the Shell
//! Command Language the check reads is exercised once: a router call in each
//! command position is reported, and the router literal in every position
//! that is no call is not. The broker implements none of the `probe-…`
//! verbs, so its refusal lists exactly the calls the grammar found.
//!
//! `BRAMA=<brama> ENTITLEMENTS_ROUTER_BIN=<pinned router>
//! WISENT_TEST_EVIDENCE_DIR=<evidence directory> cargo test --test
//! launcher-verbs -- --ignored`; the commands, exit statuses and outputs are
//! kept in `<evidence directory>/launcher-verbs/<run>/`. A Stado release
//! build names that directory itself, the qualification output it retains
//! with the release (tests/release/journey.sh).

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// Calls in every command position the grammar has; each verb names its
/// position, and the broker implements none of them.
const CALLS: &str = r#"#!/bin/sh
"$ENTITLEMENTS_ROUTER_BIN" probe-plain
CONFIG=launcher "$ENTITLEMENTS_ROUTER_BIN" probe-prefix
if "$ENTITLEMENTS_ROUTER_BIN" probe-if; then "$ENTITLEMENTS_ROUTER_BIN" probe-then; fi
while "$ENTITLEMENTS_ROUTER_BIN" probe-while; do "$ENTITLEMENTS_ROUTER_BIN" probe-do; done
for item in a b; do "$ENTITLEMENTS_ROUTER_BIN" probe-for; done
case "$item" in a|b) "$ENTITLEMENTS_ROUTER_BIN" probe-case ;; esac
! "$ENTITLEMENTS_ROUTER_BIN" probe-bang
true && "$ENTITLEMENTS_ROUTER_BIN" probe-and || "$ENTITLEMENTS_ROUTER_BIN" probe-or
printf x | "$ENTITLEMENTS_ROUTER_BIN" probe-pipe
{ "$ENTITLEMENTS_ROUTER_BIN" probe-brace; }
( "$ENTITLEMENTS_ROUTER_BIN" probe-subshell )
captured=$("$ENTITLEMENTS_ROUTER_BIN" probe-substitution)
quoted=`"$ENTITLEMENTS_ROUTER_BIN" probe-backquote`
exec "$ENTITLEMENTS_ROUTER_BIN" probe-exec \
  --after-continuation
"#;

/// The router literal where it is no call: an operand, a test operand, an
/// assignment, a redirection target, a case pattern, a loop word list, a
/// comment, a here-document, a single-quoted string and an unquoted spelling.
const DECOYS: &str = r#"brama gateway --router "$ENTITLEMENTS_ROUTER_BIN" decoy-operand
[ -x "$ENTITLEMENTS_ROUTER_BIN" ] && printf ok
ROUTER="$ENTITLEMENTS_ROUTER_BIN"
printf x > "$ENTITLEMENTS_ROUTER_BIN" decoy-redirect
case "$ROUTER" in "$ENTITLEMENTS_ROUTER_BIN"|decoy-pattern) : ;; esac
for word in "$ENTITLEMENTS_ROUTER_BIN" decoy-loop-words; do :; done
# "$ENTITLEMENTS_ROUTER_BIN" decoy-comment
cat <<EOF
"$ENTITLEMENTS_ROUTER_BIN" decoy-here-document
EOF
printf '%s' '"$ENTITLEMENTS_ROUTER_BIN" decoy-single-quoted'
$ENTITLEMENTS_ROUTER_BIN decoy-unquoted
"#;

/// Every verb [`CALLS`] and the sourced stages call, as the broker's refusal
/// lists them: sorted, joined by a comma and a space.
const EXPECTED_MISSING: &str = "probe-and, probe-backquote, probe-bang, probe-brace, probe-case, \
probe-deep-stage, probe-do, probe-exec, probe-for, probe-if, probe-or, probe-pipe, probe-plain, \
probe-prefix, probe-stage, probe-subshell, probe-substitution, probe-then, probe-while";

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
        .expect("retain launcher verbs evidence");
    }
}

impl Drop for Evidence {
    fn drop(&mut self) {
        self.report["passed"] = json!(!std::thread::panicking());
        self.save();
    }
}

fn write(path: &Path, content: &[u8]) {
    std::fs::create_dir_all(path.parent().expect("fixture inside a directory"))
        .expect("create fixture directory");
    std::fs::write(path, content).expect("write launcher fixture");
}

fn verbs(evidence: &mut Evidence, brama: &str, router: &str, launcher: &Path) -> Output {
    evidence.invoke(
        brama,
        &[
            "launcher",
            "verbs",
            "--router",
            router,
            "--launcher",
            launcher.to_str().expect("UTF-8 launcher path"),
        ],
    )
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
#[ignore = "Requires the actual Brama binary and the pinned Skarbiec router"]
fn launcher_verbs_reads_router_calls_with_the_shell_grammar() {
    let brama = std::env::var("BRAMA").expect("BRAMA must name the tested binary");
    let router = std::env::var("ENTITLEMENTS_ROUTER_BIN")
        .expect("ENTITLEMENTS_ROUTER_BIN must name the pinned Skarbiec router");
    let evidence_root = std::env::var_os("WISENT_TEST_EVIDENCE_DIR")
        .expect("WISENT_TEST_EVIDENCE_DIR must name the directory the run's evidence is kept in");
    let directory = PathBuf::from(evidence_root)
        .join("launcher-verbs")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&directory).expect("create isolated evidence directory");
    let mut evidence = Evidence {
        directory: directory.clone(),
        report: json!({ "commands": [] }),
    };
    let root = env!("CARGO_MANIFEST_DIR");
    // The revision is the one the binary under test was built from, which it
    // states itself; a release worker's source copy carries no git history.
    let version = evidence.invoke(&brama, &["version"]);
    assert!(version.status.success(), "brama version answers");
    evidence.report["build"] = json!(text(&version.stdout).trim());

    // The family: the entry point, a stage beside it, a stage one directory
    // deeper, and a file that is no stage and is not read.
    let family = directory.join("family");
    let launcher = family.join("start");
    write(&launcher, format!("{CALLS}{DECOYS}").as_bytes());
    let stages = family.join("launcher");
    write(
        &stages.join("stage.sh"),
        b"\"$ENTITLEMENTS_ROUTER_BIN\" probe-stage\n",
    );
    write(
        &stages.join("policy/deep.sh"),
        b"\"$ENTITLEMENTS_ROUTER_BIN\" probe-deep-stage\n",
    );
    write(
        &stages.join("notes.txt"),
        b"\"$ENTITLEMENTS_ROUTER_BIN\" decoy-not-a-stage\n",
    );
    let checked = verbs(&mut evidence, &brama, &router, &launcher);
    let stdout = text(&checked.stdout);
    let stderr = text(&checked.stderr);
    assert!(
        !checked.status.success(),
        "the probe verbs are refused: {stdout}{stderr}"
    );
    let required = stdout
        .lines()
        .find_map(|line| line.strip_prefix("launcher requires: "))
        .expect("the check names every command path it requires");
    for verb in EXPECTED_MISSING.split(", ") {
        assert!(
            required.split(", ").any(|path| path == verb),
            "{verb} is required: {required}"
        );
    }
    assert!(
        !required.contains("decoy-"),
        "no decoy is a call: {required}"
    );
    // The refusal is the command's last word on standard error; the waits on
    // the router's inventory calls are reported there before it.
    let refusal = format!("the pinned broker does not implement: {EXPECTED_MISSING}");
    assert_eq!(
        stderr.lines().last(),
        Some(refusal.as_str()),
        "the refusal lists exactly the calls the grammar found: {stderr}"
    );
    let refusals: Vec<&str> = stderr
        .lines()
        .filter(|line| line.starts_with("the pinned broker"))
        .collect();
    assert_eq!(
        refusals,
        [refusal.as_str()],
        "one refusal and no other: {stderr}"
    );

    // The launcher this binary ships: every call it makes is implemented.
    let shipped = Path::new(root).join("src/release/bin/start-with-skarbiec");
    let accepted = verbs(&mut evidence, &brama, &router, &shipped);
    assert!(
        accepted.status.success(),
        "the shipped launcher is accepted: {}",
        text(&accepted.stderr)
    );
    assert!(text(&accepted.stdout)
        .contains("the pinned broker advertises every literal command path the launcher invokes"));

    // A stage the grammar cannot read refuses the whole check by name.
    let broken = directory.join("broken");
    let broken_launcher = broken.join("start");
    write(&broken_launcher, CALLS.as_bytes());
    let broken_stage = broken.join("launcher/open-quote.sh");
    write(
        &broken_stage,
        b"\"$ENTITLEMENTS_ROUTER_BIN\" probe-open \"\n",
    );
    let unreadable = verbs(&mut evidence, &brama, &router, &broken_launcher);
    let refusal = text(&unreadable.stderr);
    assert!(!unreadable.status.success(), "an unreadable stage refuses");
    assert!(
        refusal.contains(broken_stage.to_str().expect("UTF-8 stage path"))
            && refusal.contains("the script ends inside"),
        "the refusal names the stage and the construct it ends inside: {refusal}"
    );
    assert!(!text(&unreadable.stdout).contains("launcher requires:"));

    // A launcher that is not text: the arguments were given in the wrong order.
    let binary = directory.join("binary/start");
    write(&binary, &[u8::MAX, u8::MAX]);
    let swapped = verbs(&mut evidence, &brama, &router, &binary);
    assert!(!swapped.status.success(), "a non-text launcher refuses");
    assert!(text(&swapped.stderr).contains("is not a launcher: it is not UTF-8 text"));

    // A launcher path with no directory to read its stages from.
    let rootless = verbs(&mut evidence, &brama, &router, Path::new("/"));
    assert!(
        !rootless.status.success(),
        "a path without a parent refuses"
    );
    assert!(text(&rootless.stderr).contains("names no file inside a directory"));
}
