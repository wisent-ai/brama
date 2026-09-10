//! One test-owned directory, and one real Skarbiec vault over data this test
//! owns.
//!
//! Brama's subscription inventory is whatever `ENTITLEMENTS_ROUTER_BIN`
//! answers, and in production that is `skarbiec-entitlements-router` or
//! `skarbiec` itself. Until this change a three-line `/bin/sh` script stood
//! there, answering `list` by printing a JSON file: a stand-in for another
//! Wisent product, which could not go out of date and so could never tell us
//! when Brama and Skarbiec disagreed. What is isolated now is the data, never
//! the component: the real binaries run against a vault this fixture creates
//! with `skarbiec init`, under its own `HOME` and `GNUPGHOME`, seeded by real
//! `skarbiec set-json` writes. Roots are short and are not `/tmp`, because
//! GnuPG binds agent sockets below `GNUPGHOME` and macOS caps `sun_path` at
//! 104 bytes.
#![allow(dead_code)]

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// GnuPG needs the short product-owned root because macOS limits Unix socket paths to 104 bytes.
fn temp_base() -> PathBuf {
    let home = std::env::var_os("HOME").expect("HOME is required");
    PathBuf::from(home).join(".brama").join("test-runs")
}

fn fixture_name(story: &str) -> String {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
    let stem: String = story
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(4)
        .collect();
    format!(
        "{stem}{:x}{:x}{sequence:x}",
        std::process::id(),
        nonce & 0xffff_ffff
    )
}

pub struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    pub fn new(story: &str) -> Self {
        let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join(fixture_name(story));
        fs::create_dir_all(&path).expect("create Brama test directory");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Resolve one sibling product's binary in the fleet's fixed order: the
/// product's own environment variable, then `PATH`, then `~/.stado/bin`.
///
/// When none holds it the test fails here, naming what is missing. There is no
/// fourth step: a scripted stand-in, an `#[ignore]` or a silent skip would
/// make this suite go green precisely when the dependency is absent.
pub fn sibling_binary(variable: &str, name: &str) -> PathBuf {
    if let Some(declared) = std::env::var_os(variable) {
        if !declared.is_empty() {
            let path = PathBuf::from(&declared);
            assert!(
                path.is_file(),
                "{variable} names {}, which is not a file: point it at the real {name} binary",
                path.display()
            );
            return path;
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&paths) {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    let home = std::env::var_os("HOME").expect("HOME is required");
    let installed = PathBuf::from(home).join(".stado").join("bin").join(name);
    if installed.is_file() {
        return installed;
    }
    panic!(
        "the real `{name}` binary is required and was not found: set {variable} to it, put it on \
         PATH, or install it at ~/.stado/bin/{name} -- this test drives the real product and has \
         no stand-in for it"
    );
}

/// One isolated Skarbiec vault, served by the real binaries.
pub struct SkarbiecVault {
    root: PathBuf,
    gnupg: PathBuf,
    vault: PathBuf,
    audit: PathBuf,
    skarbiec: PathBuf,
    router: PathBuf,
}

impl SkarbiecVault {
    /// Create and initialize one vault, owned by this test.
    pub fn create(story: &str) -> Self {
        let skarbiec = sibling_binary("SKARBIEC_BIN", "skarbiec");
        let router = sibling_binary("ENTITLEMENTS_ROUTER_BIN", "skarbiec-entitlements-router");
        let root = temp_base().join(fixture_name(story));
        let gnupg = root.join("g");
        fs::create_dir_all(&gnupg).expect("create the isolated GPG home");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
                .expect("protect the isolated fixture root");
            fs::set_permissions(&gnupg, fs::Permissions::from_mode(0o700))
                .expect("protect the isolated GPG home");
        }
        let fixture = Self {
            vault: root.join("vault.json"),
            audit: root.join("audit.jsonl"),
            root,
            gnupg,
            skarbiec,
            router,
        };
        let initialized = fixture.skarbiec(&["init", "brama-tests"]);
        assert!(
            initialized.status.success(),
            "the real skarbiec could not initialize an isolated vault: {}",
            String::from_utf8_lossy(&initialized.stderr)
        );
        assert!(
            fixture.vault.is_file(),
            "skarbiec init reported success and wrote no vault at {}",
            fixture.vault.display()
        );
        fixture
    }

    pub fn router(&self) -> &Path {
        &self.router
    }

    /// The environment pointing any process at this vault instead of the
    /// operator's. `HOME` is redirected because every Skarbiec backend falls
    /// back to a path below it, and Brama passes its own environment down to
    /// the router child, so this isolates that child too.
    pub fn environment(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("HOME", self.root.clone()),
            ("GNUPGHOME", self.gnupg.clone()),
            ("SKARBIEC_VAULT_FILE", self.vault.clone()),
            ("SKARBIEC_AUDIT_FILE", self.audit.clone()),
        ]
    }

    pub fn item_id(provider: &str, subscription_id: &str) -> String {
        format!("provider:{provider}:{subscription_id}")
    }

    /// Seed one account as a real Skarbiec item carrying the four tags
    /// discovery reads: the marker, the owning agent, the provider and the
    /// subscription id. An item missing any of them does not exist for any
    /// caller -- what `tests/routing/subscription_tag_contract.rs` is about.
    pub fn seed_subscription(&self, agent: &str, provider: &str, subscription_id: &str) -> String {
        let item = Self::item_id(provider, subscription_id);
        let tags = format!(
            "brama:subscription,brama:agent:{agent},brama:provider:{provider},\
             brama:id:{subscription_id}"
        );
        // The document shape `skarbiec set-json` accepts, and the one Brama's
        // own credential writer sends. Without the `context` object the real
        // binary refuses: "canonical item context must be an object".
        let document = serde_json::json!({
            "kind": "bundle",
            "schema": "skarbiec.item.v2",
            "context": {"source_kind": "donation"},
            "fields": {"value": format!("seeded-{subscription_id}")},
        })
        .to_string();
        let written = self.skarbiec_with_stdin(
            &["set-json", &item, "--type", "bundle", "--tags", &tags],
            &document,
        );
        assert!(
            written.status.success(),
            "the real skarbiec refused to seed {item}: {}",
            String::from_utf8_lossy(&written.stderr)
        );
        item
    }

    /// Drop one account out of the vault for real, as an operator does.
    pub fn delete_item(&self, item: &str) {
        let deleted = self.skarbiec(&["delete", item]);
        assert!(
            deleted.status.success(),
            "the real skarbiec refused to delete {item}: {}",
            String::from_utf8_lossy(&deleted.stderr)
        );
    }

    pub fn list(&self) -> Vec<Value> {
        let listed = self.skarbiec(&["list"]);
        assert!(
            listed.status.success(),
            "the real skarbiec could not list the isolated vault: {}",
            String::from_utf8_lossy(&listed.stderr)
        );
        serde_json::from_slice(&listed.stdout).expect("the real skarbiec list is a JSON array")
    }

    /// The tags one item carries, read back out of the real vault: tags are
    /// what Brama's discovery depends on, so they are what a story asserts.
    pub fn tags_of(&self, item: &str) -> Vec<String> {
        let listing = self.list();
        let row = listing
            .iter()
            .find(|row| row["id"] == item)
            .unwrap_or_else(|| panic!("{item} is not in the isolated vault: {listing:?}"));
        row["tags"]
            .as_array()
            .expect("every listed item carries a tags array")
            .iter()
            .map(|tag| tag.as_str().expect("every tag is a string").to_owned())
            .collect()
    }

    fn skarbiec(&self, args: &[&str]) -> Output {
        let mut command = Command::new(&self.skarbiec);
        command.args(args);
        self.apply_environment(&mut command);
        command.output().expect("run the real skarbiec binary")
    }

    fn skarbiec_with_stdin(&self, args: &[&str], stdin: &str) -> Output {
        let mut command = Command::new(&self.skarbiec);
        command.args(args);
        self.apply_environment(&mut command);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the real skarbiec binary");
        child
            .stdin
            .take()
            .expect("skarbiec stdin")
            .write_all(stdin.as_bytes())
            .expect("write the item document");
        child.wait_with_output().expect("collect skarbiec output")
    }

    /// Point one command at this vault and at nothing of the operator's.
    /// Dropping the whole `SKARBIEC_` prefix first is the half easy to miss:
    /// an operator shell exporting `SKARBIEC_CAPABILITY_ROUTES_FILE` or
    /// `SKARBIEC_UNLOCK` would hand this fixture the real routes table and the
    /// real passphrase.
    fn apply_environment(&self, command: &mut Command) {
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("SKARBIEC_") {
                command.env_remove(&key);
            }
        }
        for (name, value) in self.environment() {
            command.env(name, value);
        }
    }
}

impl Drop for SkarbiecVault {
    fn drop(&mut self) {
        let _ = Command::new("gpgconf")
            .env("GNUPGHOME", &self.gnupg)
            .args(["--kill", "all"])
            .status();
        let _ = fs::remove_dir_all(&self.root);
    }
}
