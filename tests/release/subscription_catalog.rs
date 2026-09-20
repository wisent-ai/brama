//! The launcher stage that decides which subscriptions this gateway serves,
//! run as it ships.
//!
//! The runtime policy is generated from the vault's own tags when a release is
//! installed. A subscription added to the vault after that install is not in
//! the policy, and until 2026-09-20 the stage dropped it without a word: on
//! charless-mac-mini three paid Claude accounts and two Codex accounts were in
//! the vault, correctly tagged, routed by `stado route capability brama`, and
//! invisible to the gateway — the only trace was `brama subscription refresh`
//! answering `no capability route maps this resource to a vault item and
//! field` hours later, and Oko's judge getting no model at all.
//!
//! The stage is run here as the launcher runs it: the real file, a real vault
//! listing, a real policy document, and the catalogue it writes read back.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// The listing `ENTITLEMENTS_ROUTER_BIN list` produces: items with the tags a
/// sign-in or an import writes.
const LISTING: &str = r#"[
  {"id": "brama-sub-wisent-app-claude-primary",
   "tags": ["brama:subscription", "brama:provider:claude-code", "brama:id:brama-sub-wisent-app-claude-primary", "brama:login:weles-google-sso-login"]},
  {"id": "brama-sub-wisent-app-codex-primary",
   "tags": ["brama:subscription", "brama:provider:codex", "brama:id:brama-sub-wisent-app-codex-primary"]},
  {"id": "brama-sub-untagged",
   "tags": ["brama:subscription", "brama:provider:kimi"]}
]"#;

/// The policy the installed release generated: it names the Claude resource
/// and not the Codex one, which is exactly the drift this defends.
const POLICY: &str = r#"{
  "roles": {
    "brama-runtime": [
      {"purpose": "brama.provider.authenticate", "resource": "provider:claude-code:brama-sub-wisent-app-claude-primary"}
    ]
  }
}"#;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(label: &str) -> PathBuf {
    let path = repo().join("target/launcher-stage").join(label);
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create the scratch directory");
    path
}

/// The stage as the launcher sources it: the same file, with the runtime and
/// config directories it reads and a router that prints the listing.
fn run_stage(label: &str) -> (String, serde_json::Value) {
    let root = scratch(label);
    let runtime = root.join("runtime");
    let config = root.join("config");
    fs::create_dir_all(&runtime).expect("runtime directory");
    fs::create_dir_all(&config).expect("config directory");
    fs::write(config.join("policy.json"), POLICY).expect("write the policy");
    let router = root.join("entitlements-router");
    fs::write(
        &router,
        format!("#!/bin/sh\ncat <<'JSON'\n{LISTING}\nJSON\n"),
    )
    .expect("write the router");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&router, fs::Permissions::from_mode(0o755))
            .expect("router is executable");
    }
    let stage = repo().join("src/release/bin/launcher/policy/subscription-catalog.sh");
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(". {}", stage.display()))
        .env("runtime_dir", &runtime)
        .env("config_dir", &config)
        .env("ENTITLEMENTS_ROUTER_BIN", &router)
        .env("PYTHON_BIN", "python3")
        .output()
        .expect("run the launcher stage");
    assert!(
        output.status.success(),
        "the stage failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let written = fs::read_to_string(runtime.join("subscription-catalog.json"))
        .expect("read the catalogue the stage wrote");
    (
        String::from_utf8_lossy(&output.stderr).to_string(),
        serde_json::from_str(&written).expect("the catalogue is JSON"),
    )
}

#[test]
fn a_subscription_the_policy_names_is_served_and_one_it_does_not_is_reported() {
    let (log, catalogue) = run_stage("named-and-unnamed");
    let items = catalogue["items"].as_array().expect("items").clone();
    let served: Vec<&str> = items
        .iter()
        .filter_map(|item| item["id"].as_str())
        .collect();
    assert_eq!(
        served,
        ["brama-sub-wisent-app-claude-primary"],
        "only the subscription the policy names is served: {served:?}"
    );
    assert_eq!(
        items[0]["login_item"].as_str(),
        Some("weles-google-sso-login"),
        "the account that can renew it travels with it"
    );

    assert!(
        log.contains("provider:codex:brama-sub-wisent-app-codex-primary"),
        "the dropped subscription is not named: {log}"
    );
    assert!(
        log.contains("no capability route maps this resource to a vault item and field"),
        "the log does not say what the gateway will answer later: {log}"
    );
    assert!(
        log.contains("install this release again on this host"),
        "the log does not name the remedy: {log}"
    );
    assert!(
        log.contains("1 subscription(s) the runtime policy does not name"),
        "the boot log has no count: {log}"
    );
    assert!(
        log.contains("1 subscription(s) served by this gateway"),
        "the boot log does not say what is served: {log}"
    );
}

#[test]
fn an_item_without_an_id_tag_says_what_is_missing() {
    let (log, _) = run_stage("untagged");
    assert!(log.contains("skipping brama-sub-untagged"), "{log}");
    assert!(
        log.contains("brama:id:"),
        "the missing tag is not named: {log}"
    );
}
