//! One stylesheet for the whole published documentation site.
//!
//! Every page under `vercel-ingress/docs` used to carry its own copy of the
//! same 102 lines of CSS. A reader downloaded them once per page, an edit to
//! the design had to be repeated seventy-two times, and five published pages
//! were longer than the repository's line limit for no reason but the copies.
//! The copies are gone; this is what keeps a new page from bringing one back.

use std::fs;
use std::path::{Path, PathBuf};

const STYLESHEET: &str = "vercel-ingress/docs/assets/site.css";
const LINK: &str = "<link rel=\"stylesheet\" href=\"/docs/assets/site.css\">";

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// Every `.html` page below `vercel-ingress/docs`, in a stable order.
fn pages(root: &Path, found: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(root)
        .expect("read documentation directory")
        .map(|entry| entry.expect("documentation directory entry").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            pages(&path, found);
        } else if path.extension().is_some_and(|kind| kind == "html") {
            found.push(path);
        }
    }
}

#[test]
fn every_published_page_links_the_one_stylesheet() {
    let repository = repository();
    let stylesheet = repository.join(STYLESHEET);
    assert!(
        stylesheet.is_file(),
        "the shared stylesheet is missing: {}",
        stylesheet.display()
    );

    let mut found = Vec::new();
    pages(&repository.join("vercel-ingress/docs"), &mut found);
    assert!(
        found.len() > 100,
        "expected the whole documentation site, found {} pages",
        found.len()
    );

    for page in found {
        let markup = fs::read_to_string(&page).expect("read documentation page");
        let name = page
            .strip_prefix(&repository)
            .unwrap_or(&page)
            .display()
            .to_string();
        assert!(
            markup.contains(LINK),
            "{name} does not link the shared stylesheet"
        );
        assert!(
            !markup.contains("<style>"),
            "{name} inlines a stylesheet instead of linking {STYLESHEET}"
        );
    }
}
