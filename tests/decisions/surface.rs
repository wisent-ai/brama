//! The same decision from a shell: `brama decide`, and the `brama routes`
//! command an operator declares the alias with.

use serde_json::{json, Value};

use crate::fixture::{
    brama, decision_route, declared_options, questions, registry, scratch, BEST_DECISION_ALIAS,
    CHOICE_QUESTION, DECISION_ALIAS, STATE,
};

#[test]
fn the_cli_decides_and_refuses_an_unacknowledged_cost() {
    let root = scratch("cli");
    let path = registry(&root, &json!({DECISION_ALIAS: decision_route()}));
    let asked = questions().to_string();

    let refused = brama(
        &path,
        &[
            "decide",
            "--model",
            DECISION_ALIAS,
            "--state",
            STATE,
            "--questions",
            &asked,
        ],
    );
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("refusing a billable decision without explicit --allow-provider-cost"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );

    let served = brama(
        &path,
        &[
            "decide",
            "--model",
            DECISION_ALIAS,
            "--state",
            STATE,
            "--questions",
            &asked,
            "--json",
            "--allow-provider-cost",
        ],
    );
    let stdout = String::from_utf8_lossy(&served.stdout);
    assert!(
        served.status.success(),
        "the CLI did not decide: {stdout}{}",
        String::from_utf8_lossy(&served.stderr)
    );
    let body: Value = serde_json::from_str(&stdout).expect("the CLI answers JSON");
    assert_eq!(body["model"], DECISION_ALIAS, "{body}");
    let options = declared_options();
    let choice = body
        .pointer(&format!("/answers/{CHOICE_QUESTION}/choice"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the CLI answered no choice: {body}"));
    assert!(
        options.iter().any(|option| option == choice),
        "the CLI answered an option the question never declared: {body}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn routes_set_declares_a_decision_alias_and_refuses_an_impossible_one() {
    let root = scratch("routes");
    let path = registry(&root, &json!({}));
    let file = path.display().to_string();
    let route = decision_route();

    let declared = brama(
        &path,
        &[
            "routes",
            "set",
            DECISION_ALIAS,
            &route,
            "--file",
            &file,
            "--json",
        ],
    );
    assert!(
        declared.status.success(),
        "{}",
        String::from_utf8_lossy(&declared.stderr)
    );
    // The file on disk, not the printed report.
    let on_disk: Value = serde_json::from_slice(&std::fs::read(&path).expect("read registry"))
        .expect("registry JSON");
    assert_eq!(
        on_disk
            .pointer(&format!("/routes/{DECISION_ALIAS}"))
            .and_then(Value::as_str),
        Some(route.as_str()),
        "{on_disk}"
    );

    // `decision-model` resolves to one route it can serve; delegation is what
    // `best-decision-model` is for, so `best` is refused here by name.
    let refused = brama(
        &path,
        &["routes", "set", DECISION_ALIAS, "best", "--file", &file],
    );
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("alias `decision-model` cannot carry `best`"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );

    // An embedding route answers no decision either.
    let refused = brama(
        &path,
        &[
            "routes",
            "set",
            BEST_DECISION_ALIAS,
            "openai/embeddings",
            "--file",
            &file,
        ],
    );
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr)
            .contains("alias `best-decision-model` cannot carry `openai/embeddings`"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );

    // What the operator declared is what `brama aliases` reads back.
    let listed = brama(&path, &["aliases", "--json"]);
    assert!(listed.status.success());
    let report: Value = serde_json::from_slice(&listed.stdout).expect("aliases JSON");
    let row = report["aliases"]
        .as_array()
        .and_then(|rows| {
            rows.iter()
                .find(|row| row["alias"] == DECISION_ALIAS)
                .cloned()
        })
        .unwrap_or_else(|| panic!("the declared alias is not in the report: {report}"));
    assert_eq!(row["route"].as_str(), Some(route.as_str()), "{report}");

    // Removing it leaves the gateway knowing the name and holding no route.
    let removed = brama(
        &path,
        &["routes", "rm", DECISION_ALIAS, "--file", &file, "--json"],
    );
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    let on_disk: Value = serde_json::from_slice(&std::fs::read(&path).expect("read registry"))
        .expect("registry JSON");
    assert!(
        on_disk
            .pointer(&format!("/routes/{DECISION_ALIAS}"))
            .is_none(),
        "{on_disk}"
    );
    let _ = std::fs::remove_dir_all(&root);
}
