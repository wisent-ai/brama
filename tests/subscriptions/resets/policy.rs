//! Run on an isolated gateway's host, using its real CLI and policy state directory.
//! No provider credit is read or consumed; only this journey's absent provider policy is changed.
use crate::support::{required, Run};
use serde_json::Value;
use std::fs::{File, OpenOptions};
use std::path::Path;

fn show(run: &mut Run) -> Value {
    let output = run.command(&["subscription", "reset-policy", "show"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document: Value = serde_json::from_slice(&output.stdout).expect("real policy response");
    assert!(
        document["policies"].is_object(),
        "policy inventory is missing"
    );
    document["policies"].clone()
}

fn lock(path: &Path) -> File {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .read(true)
        .open(path)
        .expect("open actual gateway policy lock");
    file.try_lock()
        .expect("acquire gateway policy lock for contention qualification");
    file
}

fn persisted(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("read gateway policy file"))
        .expect("decode persisted policy inventory")
}

fn refused(run: &mut Run, arguments: &[&str], provider: &str, path: &Path) {
    let output = run.command(arguments);
    assert!(
        !output.status.success(),
        "policy mutation ignored an actual held lock"
    );
    let diagnostic = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        diagnostic.contains(provider),
        "refusal omitted the affected provider: {diagnostic}"
    );
    assert!(
        diagnostic.contains(path.to_str().expect("UTF-8 lock path")),
        "refusal omitted the actual failed lock: {diagnostic}"
    );
}

#[test]
#[ignore = "Requires isolated real gateway, its local RESET_POLICY_STATE_DIR, and an absent POLICY_PROVIDER with declared reset support"]
fn policy_mutations_preserve_state_under_cross_process_contention() {
    let mut run = Run::new();
    let provider = required("POLICY_PROVIDER");
    let keep = required("POLICY_KEEP_CREDITS");
    let minutes = required("POLICY_MIN_BLOCKED_MINUTES");
    let hours = required("POLICY_SALVAGE_HORIZON_HOURS");
    let directory = std::path::PathBuf::from(required("RESET_POLICY_STATE_DIR"));
    assert!(
        directory.is_dir(),
        "use the running isolated gateway's existing state directory"
    );
    let file = directory.join("reset-policy.json");
    let lock_path = directory.join("reset-policy.lock");
    let before = show(&mut run);
    assert!(
        before.get(&provider).is_none(),
        "must not replace an existing provider policy"
    );
    let reason = format!("policy qualification {}", uuid::Uuid::new_v4());
    let arguments = [
        "subscription",
        "reset-policy",
        "set",
        &provider,
        "--keep-credits",
        &keep,
        "--min-blocked-minutes",
        &minutes,
        "--salvage-horizon-hours",
        &hours,
        "--reason",
        &reason,
    ];

    let held = lock(&lock_path);
    refused(&mut run, &arguments, &provider, &lock_path);
    assert_eq!(show(&mut run), before, "refused set changed gateway policy");
    drop(held);

    let output = run.command(&arguments);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let created = show(&mut run);
    assert_eq!(
        created[&provider]["keep_credits"],
        keep.parse::<u64>().expect("POLICY_KEEP_CREDITS")
    );
    assert_eq!(
        created[&provider]["min_blocked_minutes"],
        minutes.parse::<u64>().expect("POLICY_MIN_BLOCKED_MINUTES")
    );
    assert_eq!(
        created[&provider]["salvage_horizon_hours"],
        hours.parse::<u64>().expect("POLICY_SALVAGE_HORIZON_HOURS")
    );
    assert_eq!(created[&provider]["reason"], reason);
    assert_eq!(
        persisted(&file),
        created,
        "CLI success did not persist the policy"
    );
    let mut unrelated = created.clone();
    unrelated.as_object_mut().unwrap().remove(&provider);
    assert_eq!(unrelated, before, "set replaced another provider's policy");
    run.snapshot_pool();

    let updated_reason = format!("updated {reason}");
    let update = [
        "subscription",
        "reset-policy",
        "set",
        &provider,
        "--keep-credits",
        &keep,
        "--min-blocked-minutes",
        &minutes,
        "--salvage-horizon-hours",
        &hours,
        "--reason",
        &updated_reason,
    ];
    let held = lock(&lock_path);
    refused(&mut run, &update, &provider, &lock_path);
    assert_eq!(
        persisted(&file),
        created,
        "refused update changed persisted state"
    );
    drop(held);
    let output = run.command(&update);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let updated = show(&mut run);
    assert_eq!(updated[&provider]["reason"], updated_reason);
    assert_eq!(persisted(&file), updated);

    let clear = ["subscription", "reset-policy", "clear", &provider];
    let held = lock(&lock_path);
    refused(&mut run, &clear, &provider, &lock_path);
    assert_eq!(
        show(&mut run),
        updated,
        "refused clear changed gateway policy"
    );
    assert_eq!(persisted(&file), updated);
    drop(held);
    let output = run.command(&clear);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(show(&mut run), before);
    assert_eq!(persisted(&file), before);
    let repeated = run.command(&clear);
    assert!(
        !repeated.status.success(),
        "clearing an absent policy must refuse"
    );
    assert_eq!(persisted(&file), before);
    run.snapshot_pool();
    run.report["policy_before"] = before;
    run.report["policy_created"] = created;
    run.report["policy_updated"] = updated;
    run.report["result"] = serde_json::json!("passed");
    run.save();
}
