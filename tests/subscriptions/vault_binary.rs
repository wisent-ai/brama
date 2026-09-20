//! Which program this gateway shells for a credential operation, and what it
//! says when it can shell none.
//!
//! # What went wrong
//!
//! Every credential this gateway reads, writes or lists goes through one
//! child process, and the program it ran was the bare word
//! `entitlements-router` whenever no environment variable declared another.
//! No machine in this fleet installs that name: the vault CLI is `skarbiec`,
//! on `PATH` and at `~/.stado/bin/skarbiec`. On 2026-09-20
//! `brama subscription sync` refused all three grants this machine holds with
//! `the grant could not be stored: list vault tags for credential write: No
//! such file or directory (os error 2)`, naming neither the missing program
//! nor a way to declare one, and the pool stayed at zero live credentials
//! while Oko's verification answered HTTP 503 for want of a working
//! subscription.
//!
//! # What is defended here
//!
//! The resolution order, driven through the real binary with an isolated home
//! and `PATH`: the router's declaration, the vault's declaration, either
//! program name on `PATH`, the installed path under `$HOME`, and — when a
//! host carries none of them — a refusal that names the program it ran and
//! both variables.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A stub vault: it records that it was the program Brama chose, answers
/// `list` with an empty inventory and `get <item>` with a Skarbiec v2 item
/// whose `token` field is `token`.
fn stub(directory: &Path, name: &str) -> PathBuf {
    stub_with_token(directory, name, "reauth-token")
}

/// The same stub, with the `token` field this test wants it to answer.
fn stub_with_token(directory: &Path, name: &str, token: &str) -> PathBuf {
    let path = directory.join(name);
    let marker = directory.join(format!("{name}.ran"));
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\n\
             if [ \"$1\" = get ]; then\n\
             \x20 printf '{{\"schema\":\"skarbiec.item.v2\",\"fields\":{{\"token\":\"{token}\"}}}}\\n'\n\
             else\n\
             \x20 printf '[]\\n'\n\
             fi\n",
            marker.display()
        ),
    )
    .expect("write the stub vault program");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make the stub executable");
    }
    path
}

fn ran(path: &Path) -> bool {
    let marker = PathBuf::from(format!("{}.ran", path.display()));
    marker.exists()
}

/// One fixture directory inside this package's own build tree.
fn directory(story: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(story);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create the fixture directory");
    root
}

/// The report command, with nothing of the operator's environment in it.
fn subscriptions(home: &Path, path_value: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    command
        .arg("subscriptions")
        .env_clear()
        .env("HOME", home)
        .env("PATH", path_value);
    command
}

#[test]
fn the_vault_declaration_is_used_when_the_router_declares_nothing() {
    let root = directory("vault-binary-skarbiec-bin");
    let home = root.join("home");
    std::fs::create_dir_all(&home).expect("an isolated home");
    let program = stub(&root, "declared-vault");

    let output = subscriptions(&home, "/nonexistent")
        .env("SKARBIEC_BIN", &program)
        .output()
        .expect("the report runs");

    assert!(
        ran(&program),
        "SKARBIEC_BIN must be the program that was run: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("No such file or directory"),
        "a declared vault program must not read as missing: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn the_router_declaration_still_wins_over_the_vault_declaration() {
    let root = directory("vault-binary-router-first");
    let home = root.join("home");
    std::fs::create_dir_all(&home).expect("an isolated home");
    let router = stub(&root, "declared-router");
    let vault = stub(&root, "declared-vault");

    subscriptions(&home, "/nonexistent")
        .env("ENTITLEMENTS_ROUTER_BIN", &router)
        .env("SKARBIEC_BIN", &vault)
        .output()
        .expect("the report runs");

    assert!(ran(&router), "the router declaration is read first");
    assert!(
        !ran(&vault),
        "the vault declaration must not be read when the router declares one"
    );
}

#[test]
fn the_installed_vault_on_path_is_found_without_any_declaration() {
    let root = directory("vault-binary-on-path");
    let home = root.join("home");
    std::fs::create_dir_all(&home).expect("an isolated home");
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).expect("a PATH directory");
    let program = stub(&bin, "skarbiec");

    subscriptions(&home, bin.to_str().expect("utf-8 PATH"))
        .output()
        .expect("the report runs");

    assert!(
        ran(&program),
        "an installed vault CLI on PATH is the program this gateway runs"
    );
}

#[test]
fn the_fleets_install_path_is_the_last_place_looked() {
    let root = directory("vault-binary-installed-path");
    let home = root.join("home");
    let installed_dir = home.join(".stado/bin");
    std::fs::create_dir_all(&installed_dir).expect("the fleet's install directory");
    let program = stub(&installed_dir, "skarbiec");

    subscriptions(&home, "/nonexistent")
        .output()
        .expect("the report runs");

    assert!(
        ran(&program),
        "the installed path under HOME is used when nothing declares or PATHs one"
    );
}

#[test]
fn a_host_with_no_vault_program_says_what_it_ran_and_what_to_declare() {
    let root = directory("vault-binary-absent");
    let home = root.join("home");
    std::fs::create_dir_all(&home).expect("an isolated home");

    let output = subscriptions(&home, "/nonexistent")
        .output()
        .expect("the report runs");
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        report.contains("No such file or directory"),
        "the operating system's own error is kept: {report}"
    );
    assert!(
        report.contains("running entitlements-router"),
        "the refusal must name the program it ran: {report}"
    );
    assert!(
        report.contains("ENTITLEMENTS_ROUTER_BIN") && report.contains("SKARBIEC_BIN"),
        "the refusal must name both declarations: {report}"
    );
}

/// The sign-in command, with the Weles endpoint declared so the story is
/// about the admission token and not about resolving a host.
fn sign_in(home: &Path, program: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brama"));
    command
        .args([
            "subscription",
            "sign-in",
            "claude-code",
            "--subscription-id",
            "brama-sub-wisent-app-claude-secondary",
            "--reason",
            "the vault answers the admission token",
        ])
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/nonexistent")
        .env("BRAMA_WELES_URL", "http://127.0.0.1:1")
        .env("SKARBIEC_BIN", program);
    command
}

/// The repair this gateway names for a grant the provider will not refresh
/// again is `subscription sign-in`, and until the Weles admission token could
/// be read from the vault that command answered
/// `BRAMA_WELES_REAUTH_TOKEN is unavailable` to everyone but the service
/// launcher, which exports it — so the person holding the refusal could never
/// perform the repair.
#[test]
fn the_weles_admission_token_is_read_from_the_vault_without_the_launcher() {
    let root = directory("vault-binary-reauth-token");
    let home = root.join("home");
    std::fs::create_dir_all(&home).expect("an isolated home");
    let program = stub(&root, "vault-with-token");

    let output = sign_in(&home, &program).output().expect("the sign-in runs");
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        ran(&program),
        "the vault program must be the one asked: {report}"
    );
    assert!(
        !report.contains("BRAMA_WELES_REAUTH_TOKEN"),
        "a vault that answers the item must satisfy the admission token: {report}"
    );
}

/// An item that answers with nothing is not a token, and the refusal says
/// which field was empty instead of failing inside Weles.
#[test]
fn an_empty_admission_token_is_refused_by_name() {
    let root = directory("vault-binary-empty-token");
    let home = root.join("home");
    std::fs::create_dir_all(&home).expect("an isolated home");
    let program = stub_with_token(&root, "vault-empty-token", "");

    let output = sign_in(&home, &program).output().expect("the sign-in runs");
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        report.contains("brama-weles-reauth/token is empty"),
        "an empty field is refused by name: {report}"
    );
}
