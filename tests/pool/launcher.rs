use std::path::Path;
use std::process::Command;

use reqwest::Method;

use super::{assert_pool_answered, Gateway, POOL, VAULT};

#[test]
fn gateway_launch_stage_execs_one_process_and_refuses_different_broker_state_without_stopping_it() {
    let gateway = Gateway::start("shared-launcher", VAULT);
    let (status, pool) = gateway.console(POOL, Method::GET, None);
    assert_eq!(status, 200, "{pool}");
    assert_pool_answered(&pool);

    let owner = Command::new("lsof")
        .args([
            "-nP",
            "-a",
            "-p",
            &gateway.pid().to_string(),
            "-iTCP",
            "-sTCP:LISTEN",
            "-Fn",
        ])
        .output()
        .expect("inspect actual gateway listener ownership");
    assert!(
        owner.status.success(),
        "{}",
        String::from_utf8_lossy(&owner.stderr)
    );
    let endpoint = gateway.origin().strip_prefix("http://").unwrap();
    let owners = String::from_utf8_lossy(&owner.stdout);
    assert!(
        owners
            .lines()
            .any(|line| line.strip_prefix('n') == Some(endpoint)),
        "{owners}"
    );

    let children = Command::new("pgrep")
        .args(["-P", &gateway.pid().to_string()])
        .output()
        .expect("inspect gateway child processes");
    assert_eq!(
        children.status.code(),
        Some(1),
        "launcher left children: {}",
        String::from_utf8_lossy(&children.stdout)
    );

    let foreign_state = gateway.root().join("foreign-capabilities.json");
    let mut refused = Command::new("sh");
    refused.arg("-eu");
    refused
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/release/bin/launcher/gateway-launch.sh"),
        )
        .arg("version");
    gateway.vault().apply_environment(&mut refused);
    let refused = refused
        .env("BRAMA_BIN", env!("CARGO_BIN_EXE_brama"))
        .env("ENTITLEMENTS_ROUTER_BIN", gateway.vault().router())
        .env("SKARBIEC_CAP_SOCKET", gateway.capability_socket())
        .env("SKARBIEC_CAPABILITY_FILE", &foreign_state)
        .output()
        .expect("exercise actual launcher state mismatch refusal");
    assert!(!refused.status.success());
    let diagnostic = String::from_utf8_lossy(&refused.stderr);
    assert!(diagnostic.contains("different state paths"), "{diagnostic}");
    assert!(!foreign_state.exists());

    let still_serving = gateway.vault().skarbiec(&[
        "capability-status",
        "--socket",
        gateway.capability_socket().to_str().unwrap(),
    ]);
    assert!(
        still_serving.status.success(),
        "{}",
        String::from_utf8_lossy(&still_serving.stderr)
    );
    let (status, pool) = gateway.console(POOL, Method::GET, None);
    assert_eq!(status, 200, "{pool}");
    assert_pool_answered(&pool);
}
