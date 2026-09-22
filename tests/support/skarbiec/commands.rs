//! Running the real Skarbiec binary against the isolated vault, and
//! reading the vault back.
//!
//! Every call scrubs the operator's `SKARBIEC_` environment first. That is
//! the half easy to miss: a shell exporting `SKARBIEC_CAPABILITY_ROUTES_FILE`
//! or `SKARBIEC_UNLOCK` would hand this fixture the real routes table and
//! the real passphrase.

use super::*;

impl SkarbiecVault {
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

    /// The document one item holds, read back out of the real vault: what a
    /// credential write left behind is what the refresh path will read.
    pub fn document_of(&self, item: &str) -> Value {
        let shown = self.skarbiec(&["get", item]);
        assert!(
            shown.status.success(),
            "the real skarbiec could not read {item}: {}",
            String::from_utf8_lossy(&shown.stderr)
        );
        serde_json::from_slice(&shown.stdout).expect("the real skarbiec get is a JSON document")
    }

    /// Put one item's document back the way an earlier writer left it, tags
    /// kept: how a story reproduces an item shape the product no longer
    /// writes, such as a grant without the account beside it.
    pub fn replace_document(&self, item: &str, document: &Value) {
        let tags = self.tags_of(item).join(",");
        let kind = document["kind"]
            .as_str()
            .expect("every canonical document names its kind")
            .to_owned();
        let written = self.skarbiec_with_stdin(
            &["set-json", item, "--type", &kind, "--tags", &tags],
            &document.to_string(),
        );
        assert!(
            written.status.success(),
            "the real skarbiec could not replace {item}: {}",
            String::from_utf8_lossy(&written.stderr)
        );
    }

    pub(crate) fn skarbiec(&self, args: &[&str]) -> Output {
        let mut command = Command::new(&self.skarbiec);
        command.args(args);
        self.apply_environment(&mut command);
        command.output().expect("run the real skarbiec binary")
    }

    pub(crate) fn skarbiec_with_stdin(&self, args: &[&str], stdin: &str) -> Output {
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
    pub(crate) fn apply_environment(&self, command: &mut Command) {
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
