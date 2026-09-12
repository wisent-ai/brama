//! The manual Claude sign-in, driven as an operator drives it.
//!
//! On 2026-09-12 every Claude subscription in the pool held no credential,
//! the only account that worked had been signed into the harness by hand, and
//! Brama had no way to take that same sign-in. These stories run the real
//! binary and read what it prints and leaves: the page it asks the operator
//! to open, what it makes of the paste, and what it refuses before touching
//! anything. The login itself is the operator's own browser and their own
//! account, and is not reproduced here.

use std::io::Write;
use std::process::{Command, Stdio};

/// The port the provider registered for this client id; the harness
/// descriptor the sign-in mirrors names it beside its callback path.
const CALLBACK_PORT: u16 = 54545;

fn manual(args: &[&str], stdin: &str) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_brama"))
        .args(["subscription", "sign-in-manual"])
        .args(args)
        .env("BRAMA_LOCAL_PROVIDER_CREDENTIALS", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the real brama binary");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write the paste");
    let output = child.wait_with_output().expect("collect output");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// The page the operator is sent to is the harness's page: same client,
/// same scopes, same loopback redirect, PKCE. A different page would yield a
/// different kind of grant, and the refresh path would not know it.
#[test]
fn the_authorize_page_is_the_one_the_harness_opens() {
    let (_, _, stderr) = manual(
        &[
            "claude-code",
            "--subscription-id",
            "brama-sub-test-claude",
            "--reason",
            "story",
        ],
        "",
    );
    let url = stderr
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("https://claude.ai/oauth/authorize?"))
        .unwrap_or_else(|| panic!("no authorize page was printed:\n{stderr}"));
    let parsed = url::Url::parse(url).expect("the printed page is a URL");
    let query: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    assert_eq!(query["client_id"], "9d1c250a-e61b-44d9-88ed-5944d1962f5e");
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["code"], "true");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(
        query["redirect_uri"],
        format!("http://localhost:{CALLBACK_PORT}/callback")
    );
    assert_eq!(
        query["scope"],
        "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload"
    );
    assert!(
        !query["state"].is_empty(),
        "a state is drawn for every sign-in"
    );
    assert!(
        !query["code_challenge"].is_empty(),
        "a PKCE challenge is drawn for every sign-in"
    );
}

/// An empty paste ends the sign-in with a sentence, not a hang or a panic.
#[test]
fn an_empty_paste_is_refused_by_name() {
    let (status, _, stderr) = manual(
        &[
            "claude-code",
            "--subscription-id",
            "brama-sub-test-claude",
            "--reason",
            "story",
        ],
        "\n",
    );
    assert_eq!(status, 1);
    assert!(stderr.contains("nothing was pasted"), "{stderr}");
}

/// A code carrying a state this sign-in did not start with is refused: it is
/// somebody else's login, or a stale page.
#[test]
fn a_paste_with_a_foreign_state_is_refused() {
    let (status, _, stderr) = manual(
        &[
            "claude-code",
            "--subscription-id",
            "brama-sub-test-claude",
            "--reason",
            "story",
        ],
        "some-code#not-this-sign-ins-state\n",
    );
    assert_eq!(status, 1);
    assert!(
        stderr.contains("not the one this sign-in started with"),
        "{stderr}"
    );
}

/// Only claude-code has a manual flow; the refusal names that.
#[test]
fn another_provider_is_refused_before_anything_is_opened() {
    let (status, _, stderr) = manual(
        &[
            "codex",
            "--subscription-id",
            "brama-sub-test-codex",
            "--reason",
            "story",
        ],
        "",
    );
    assert_eq!(status, 1);
    assert!(stderr.contains("`codex` is not it"), "{stderr}");
    assert!(
        !stderr.contains("https://"),
        "no page is printed for a provider without a manual flow"
    );
}

/// A sign-in with no reason is refused before the page is printed, like every
/// other credential change in this product.
#[test]
fn a_missing_reason_is_refused() {
    let (status, _, stderr) = manual(
        &[
            "claude-code",
            "--subscription-id",
            "brama-sub-test-claude",
            "--reason",
            "  ",
        ],
        "",
    );
    assert_eq!(status, 1);
    assert!(stderr.contains("--reason must say why"), "{stderr}");
    assert!(!stderr.contains("https://"));
}
