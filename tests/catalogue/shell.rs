//! The operator shell: what `brama models` lists and what `brama categories`
//! declares.
//!
//! Every story here drives the built binary against the real public model
//! catalogue and an owner-only route registry inside this package's own
//! build directory. Nothing touches the operator's registry, and nothing is
//! asserted about a model this fleet does not actually carry: the catalogue
//! is a live document, so the stories assert its shape — that the generated
//! kinds exist and carry the facets — rather than pinning ids a vendor may
//! retire tomorrow.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

use brama::providers::adapter::ModelKind;

use crate::support::TestDirectory;

/// The catalogue is fetched over the network on a cold run.
const CATALOGUE_TTL_SECONDS: &str = "120";

/// The kinds that are not text: what Brama could not name before this
/// change. They are the enum's own members rather than written-out words, so
/// a kind added to the product joins this story by compiling.
const GENERATED_KINDS: [ModelKind; 3] = [ModelKind::Image, ModelKind::Video, ModelKind::Audio];

struct Shell {
    directory: TestDirectory,
    registry: PathBuf,
}

impl Shell {
    fn new(story: &str) -> Self {
        let directory = TestDirectory::new(story);
        let registry = directory.path().join("inference-routes.json");
        std::fs::write(&registry, b"{\"schema_version\":1,\"routes\":{}}")
            .expect("write the isolated route registry");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&registry, std::fs::Permissions::from_mode(0o600))
                .expect("protect the isolated route registry");
        }
        Self {
            directory,
            registry,
        }
    }

    fn run(&self, arguments: &[&str]) -> (bool, String, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_brama"))
            .args(arguments)
            .env("BRAMA_INFERENCE_ROUTES_FILE", &self.registry)
            .env("HOME", self.directory.path())
            .env("BRAMA_MODEL_CATALOG_TTL_SECONDS", CATALOGUE_TTL_SECONDS)
            .output()
            .expect("run the real brama binary");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        )
    }

    fn json(&self, arguments: &[&str]) -> Value {
        let (ok, stdout, stderr) = self.run(arguments);
        assert!(ok, "{arguments:?} must succeed: {stderr}");
        serde_json::from_str(&stdout).expect("a JSON document on stdout")
    }

    fn registry_document(&self) -> Value {
        let body = std::fs::read_to_string(&self.registry).expect("read back the registry");
        serde_json::from_str(&body).expect("the registry is JSON")
    }
}

/// The two questions the catalogue could not answer before: what does this
/// model produce, and are its weights published.
#[test]
fn the_catalogue_lists_every_generated_kind_with_its_weights() {
    let shell = Shell::new("catalogue-kinds");
    for kind in GENERATED_KINDS {
        let listing = shell.json(&["models", "--kind", kind.as_str(), "--limit", "20", "--json"]);
        let models = listing["models"].as_array().expect("a model array");
        assert!(
            !models.is_empty(),
            "the public catalogue carries {} models: {listing}",
            kind.as_str()
        );
        for model in models {
            assert_eq!(
                model["kind"],
                kind.as_str(),
                "this filter answers only its own kind: {model}"
            );
        }
        assert!(
            listing["matched"].as_u64().unwrap_or_default() >= models.len() as u64,
            "the listing states how many matched: {listing}"
        );
    }

    let open = shell.json(&["models", "--weights", "open", "--limit", "20", "--json"]);
    for model in open["models"].as_array().expect("a model array") {
        assert_eq!(
            model["openWeights"], true,
            "open weights means the catalogue said so: {model}"
        );
    }
    let closed = shell.json(&["models", "--weights", "closed", "--limit", "20", "--json"]);
    for model in closed["models"].as_array().expect("a model array") {
        assert_eq!(
            model["openWeights"], false,
            "closed weights means the catalogue said so: {model}"
        );
    }
}

/// A filter this gateway has no meaning for is refused in the same words the
/// HTTP surface uses, rather than answering the whole catalogue.
#[test]
fn an_unreadable_filter_is_refused_by_name() {
    let shell = Shell::new("catalogue-refusals");
    let (ok, _, stderr) = shell.run(&["models", "--kind", "hologram"]);
    assert!(!ok, "an unknown kind must not answer a listing");
    assert!(
        stderr.contains("kind must be text, image, video or audio"),
        "{stderr}"
    );
    let (ok, _, stderr) = shell.run(&["models", "--weights", "maybe"]);
    assert!(!ok, "an unknown weights value must not answer a listing");
    assert!(
        stderr.contains("weights must be open or closed"),
        "{stderr}"
    );
}

/// The uncensored category, end to end: declared, counted, filtered on, and
/// retired. The count is the check on the declaration — a rule that matches
/// nothing is visible as zero rather than as a working facet.
#[test]
fn a_declared_category_is_written_counted_and_retired() {
    let shell = Shell::new("catalogue-categories");
    let (ok, stdout, stderr) = shell.run(&[
        "categories",
        "set",
        "uncensored",
        "--term",
        "uncensored",
        "--term",
        "abliterated",
    ]);
    assert!(ok, "declaring a category must succeed: {stderr}");
    assert!(
        stdout.contains("declared model category uncensored"),
        "{stdout}"
    );

    let document = shell.registry_document();
    assert_eq!(
        document["categories"]["uncensored"]["terms"],
        serde_json::json!(["uncensored", "abliterated"]),
        "the registry holds exactly what was declared: {document}"
    );

    let listing = shell.json(&["categories", "ls", "--json"]);
    let declared = listing["categories"].as_array().expect("a category array");
    let uncensored = declared
        .iter()
        .find(|entry| entry["category"] == "uncensored")
        .expect("the declared category is listed");
    let held = uncensored["models"].as_u64().expect("a model count");
    assert!(
        held > 0,
        "the catalogue carries models published under that name: {uncensored}"
    );

    let models = shell.json(&[
        "models",
        "--category",
        "uncensored",
        "--limit",
        "20",
        "--json",
    ]);
    let rows = models["models"].as_array().expect("a model array");
    assert!(
        !rows.is_empty(),
        "the category filters the listing: {models}"
    );
    for model in rows {
        assert!(
            model["categories"]
                .as_array()
                .is_some_and(|names| names.iter().any(|name| name == "uncensored")),
            "every listed model carries the category: {model}"
        );
    }

    let (ok, stdout, stderr) = shell.run(&["categories", "rm", "uncensored"]);
    assert!(ok, "retiring a declared category must succeed: {stderr}");
    assert!(
        stdout.contains("retired model category uncensored"),
        "{stdout}"
    );
    let document = shell.registry_document();
    assert!(
        document["categories"]
            .as_object()
            .is_none_or(|declared| !declared.contains_key("uncensored")),
        "the retired category is gone from the registry: {document}"
    );

    let (ok, _, stderr) = shell.run(&["categories", "rm", "uncensored"]);
    assert!(!ok, "retiring a category nobody declared must be refused");
    assert!(
        stderr.contains("no model category 'uncensored' is declared"),
        "{stderr}"
    );
}

/// A category name the registry cannot hold is refused before anything is
/// written, and the refusal names the shape it needed.
#[test]
fn a_malformed_category_is_refused_before_the_registry_changes() {
    let shell = Shell::new("catalogue-bad-name");
    let (ok, _, stderr) = shell.run(&["categories", "set", "Uncensored", "--term", "x"]);
    assert!(!ok, "an uppercase category name must be refused");
    assert!(
        stderr.contains("must be lowercase letters, digits and hyphens"),
        "{stderr}"
    );
    let (ok, _, stderr) = shell.run(&["categories", "set", "empty"]);
    assert!(!ok, "a category with no rule must be refused");
    assert!(
        stderr.contains("declares no providers, routes or terms"),
        "{stderr}"
    );
    let document = shell.registry_document();
    assert!(
        document.get("categories").is_none(),
        "a refused declaration leaves the registry untouched: {document}"
    );
}
