//! Nothing published from this repository carries a person's identity.
//!
//! This repository is public. On 2026-09-21 a real account count was copied
//! out of a live deployment into the CLI reference, the attribute page and
//! three test fixtures, so the operator's subscription addresses and a home
//! directory path were pushed to a public remote; the changelog and the
//! aliases page had been carrying the same shapes since 2026-09-18.
//!
//! The rule is positive rather than a list of the addresses that leaked: an
//! address published here must sit in a domain that carries the `example`
//! label the standards reserve for documentation -- `example.com` and
//! `example.invalid` both do, a real mailbox does not -- and a filesystem
//! example must not name a home directory. A list of the addresses that
//! escaped would only ever refuse those; this refuses the next one.
//!
//! Scope is everything a reader of the repository can see: the published
//! site, the source, the tests and the repository's own documents. A file
//! that is not readable as text is skipped, which is how the site's images
//! and any archived evidence stay out of it.

use std::fs;
use std::path::{Path, PathBuf};

/// The label the standards reserve for documentation. A domain carrying it
/// belongs to nobody, which is the whole property this rule needs.
const RESERVED_LABEL: &str = "example";

/// Where a published address or path can appear: the site, the code, the
/// tests and the repository's own documents.
const PUBLISHED: [&str; 5] = ["vercel-ingress/docs", "src", "tests", "README.md", "docs"];

/// The home-directory prefixes an example must not name. `~` is the way to
/// write one.
const HOME_PREFIXES: [&str; 2] = ["/Users/", "/home/"];

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// This file states the rule, so it necessarily writes down the shapes the
/// rule refuses. It is the one file the scan skips, by its own path.
fn states_the_rule(file: &Path) -> bool {
    file == Path::new(file!()) || file.ends_with(file!())
}

fn published_files(root: &Path, found: &mut Vec<PathBuf>) {
    if root.is_file() {
        found.push(root.to_path_buf());
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut paths: Vec<_> = entries
        .map(|entry| entry.expect("published directory entry").path())
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            published_files(&path, found);
        } else {
            found.push(path);
        }
    }
}

/// The addresses one published file carries: every `local@domain` run, taken
/// from the characters an address is made of rather than from a parser
/// nobody here needs.
fn addresses(text: &str) -> Vec<String> {
    let addressable = |character: char| {
        character.is_ascii_alphanumeric() || ".-_+".contains(character) || character == '@'
    };
    text.split(|character: char| !addressable(character))
        .filter(|run| run.matches('@').count() == 1)
        .filter(|run| {
            let (local, domain) = run.split_once('@').unwrap_or_default();
            !local.is_empty() && domain.contains('.') && !domain.ends_with('.')
        })
        .map(str::to_owned)
        .collect()
}

/// Whether one address sits in a domain reserved for documentation.
fn reserved(address: &str) -> bool {
    address
        .split_once('@')
        .map(|(_, domain)| domain.to_ascii_lowercase())
        .unwrap_or_default()
        .split('.')
        .any(|label| label == RESERVED_LABEL)
}

fn published() -> Vec<PathBuf> {
    let repository = repository();
    let mut files = Vec::new();
    for area in PUBLISHED {
        published_files(&repository.join(area), &mut files);
    }
    files.retain(|file| !states_the_rule(file));
    files
}

fn named_in(file: &Path) -> String {
    file.strip_prefix(repository())
        .unwrap_or(file)
        .display()
        .to_string()
}

#[test]
fn every_published_address_is_in_a_reserved_domain() {
    let files = published();
    assert!(
        files.len() > 100,
        "expected the published repository, found {} files",
        files.len()
    );
    let mut found: Vec<String> = Vec::new();
    for file in &files {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        for address in addresses(&text) {
            if !reserved(&address) {
                found.push(format!("{}: {address}", named_in(file)));
            }
        }
    }
    assert!(
        found.is_empty(),
        "a published address must sit in a domain carrying the reserved `{RESERVED_LABEL}` \
         label; found:\n{}",
        found.join("\n")
    );
}

#[test]
fn no_published_example_names_a_home_directory() {
    let mut found: Vec<String> = Vec::new();
    for file in &published() {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            if HOME_PREFIXES.iter().any(|prefix| line.contains(prefix)) {
                found.push(format!(
                    "{}:{}: {}",
                    named_in(file),
                    number.saturating_add(1),
                    line.trim()
                ));
            }
        }
    }
    assert!(
        found.is_empty(),
        "a published example names a home directory; write `~` instead:\n{}",
        found.join("\n")
    );
}
