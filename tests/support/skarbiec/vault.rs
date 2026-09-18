//! Creating one isolated vault with the real binaries, and the two writes
//! a story performs against it.
//!
//! Roots are short and are not `/tmp`, because GnuPG binds agent sockets
//! below `GNUPGHOME` and macOS caps `sun_path` at 104 bytes.

use super::*;

/// The fixture root and its GnuPG home are owner-only: a vault another
/// account can read is not an isolated vault.
#[cfg(unix)]
const PRIVATE_DIRECTORY: u32 = 0o700;

/// One isolated Skarbiec vault, served by the real binaries.
pub struct SkarbiecVault {
    pub(crate) root: PathBuf,
    pub(crate) gnupg: PathBuf,
    pub(crate) vault: PathBuf,
    pub(crate) audit: PathBuf,
    pub(crate) skarbiec: PathBuf,
    pub(crate) router: PathBuf,
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
            fs::set_permissions(&root, fs::Permissions::from_mode(PRIVATE_DIRECTORY))
                .expect("protect the isolated fixture root");
            fs::set_permissions(&gnupg, fs::Permissions::from_mode(PRIVATE_DIRECTORY))
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
    /// operator's. `HOME` is redirected because every Skarbiec backend reads
    /// a path below it, and Brama passes its own environment down to the
    /// router child, so this isolates that child too.
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

    /// Seed one account as a real Skarbiec item carrying the three tags
    /// discovery reads - the marker, the provider and the subscription id -
    /// plus the agent's provenance tag.
    pub fn seed_subscription(&self, agent: &str, provider: &str, subscription_id: &str) -> String {
        self.seed_subscription_with_document(
            agent,
            provider,
            subscription_id,
            &format!("seeded-{subscription_id}"),
        )
    }

    /// Seed one account as [`Self::seed_subscription`] does, holding `document`
    /// as its credential value: the OAuth document a story wants the request
    /// path to read, rather than the placeholder string.
    pub fn seed_subscription_with_document(
        &self,
        agent: &str,
        provider: &str,
        subscription_id: &str,
        document: &str,
    ) -> String {
        self.seed_item(
            provider,
            subscription_id,
            &format!(
                "brama:subscription,brama:agent:{agent},brama:provider:{provider},\
                 brama:id:{subscription_id}"
            ),
            document,
        )
    }

    /// Seed one account tagged for no agent at all: marked and named, which
    /// since 2026-09-16 is everything routing needs.
    pub fn seed_marked_subscription(&self, provider: &str, subscription_id: &str) -> String {
        self.seed_item(
            provider,
            subscription_id,
            &format!("brama:subscription,brama:provider:{provider},brama:id:{subscription_id}"),
            &format!("seeded-{subscription_id}"),
        )
    }

    fn seed_item(&self, provider: &str, subscription_id: &str, tags: &str, value: &str) -> String {
        let item = Self::item_id(provider, subscription_id);
        // The document shape `skarbiec set-json` accepts, and the one Brama's
        // own credential writer sends. Without the `context` object the real
        // binary refuses: "canonical item context must be an object".
        let document = serde_json::json!({
            "kind": "bundle",
            "schema": "skarbiec.item.v2",
            "context": {"source_kind": "donation"},
            "fields": {"value": value},
        })
        .to_string();
        let written = self.skarbiec_with_stdin(
            &["set-json", &item, "--type", "bundle", "--tags", tags],
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
